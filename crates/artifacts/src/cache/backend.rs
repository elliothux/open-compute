use super::*;

impl ArtifactCache {
    /// Create or open a cache at `root`, cleaning stale partial files.
    pub fn open(
        root: PathBuf,
        config: CacheConfig,
        startup_id: StartupId,
    ) -> Result<Self, PlatformError> {
        validate_cache_root(&root)?;
        ensure_real_dir(&root)?;
        let sha_root = root.join("sha256");
        ensure_child_dir(&root, "sha256")?;
        cleanup_stale_partials(&sha_root, Duration::from_millis(config.partial_grace_ms));
        let inner = rebuild_index(&sha_root);
        Ok(Self {
            root,
            config,
            startup_id,
            inner: Arc::new(Mutex::new(inner)),
            inflight: AsyncMutex::new(HashMap::new()),
        })
    }

    /// Index an existing cache directory without creating files or cleaning partials.
    pub fn inspect_existing(root: PathBuf) -> Result<Self, PlatformError> {
        validate_cache_root(&root)?;
        let meta = fs::symlink_metadata(&root).map_err(|_| {
            PlatformError::new(
                ErrorCode::PathInvalid,
                "artifact cache directory is missing",
            )
        })?;
        if meta.file_type().is_symlink() || !meta.file_type().is_dir() {
            return Err(PlatformError::new(
                ErrorCode::PathInvalid,
                "artifact cache directory is missing",
            ));
        }
        let sha_root = root.join("sha256");
        let inner = match fs::symlink_metadata(&sha_root) {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(PlatformError::new(
                    ErrorCode::PathInvalid,
                    "cache shard root must not be a symlink",
                ));
            }
            Ok(meta) if meta.file_type().is_dir() => rebuild_index(&sha_root),
            Ok(_) => {
                return Err(PlatformError::new(
                    ErrorCode::PathInvalid,
                    "cache shard root must be a directory",
                ));
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => CacheInner {
                entries: HashMap::new(),
                lru: VecDeque::new(),
                total_bytes: 0,
            },
            Err(_) => {
                return Err(PlatformError::new(
                    ErrorCode::PathInvalid,
                    "cache shard root is not accessible",
                ));
            }
        };
        Ok(Self {
            root,
            config: CacheConfig::default(),
            startup_id: StartupId::generate(),
            inner: Arc::new(Mutex::new(inner)),
            inflight: AsyncMutex::new(HashMap::new()),
        })
    }

    /// Fetch or open a pinned, verified artifact.
    pub async fn acquire(
        &self,
        store: &ArtifactStore,
        artifact: &ArtifactRef,
    ) -> Result<PinnedArtifact, PlatformError> {
        match self.try_hit(artifact) {
            Ok(Some(hit)) => return Ok(hit),
            Ok(None) => {}
            Err(err)
                if err.code() == ErrorCode::CacheEntryCorrupt
                    || err.code() == ErrorCode::ArtifactIntegrityError =>
            {
                self.quarantine(artifact);
            }
            Err(err) => return Err(err),
        }
        self.singleflight_fetch(store, artifact).await?;
        match self.try_hit(artifact) {
            Ok(Some(hit)) => Ok(hit),
            Ok(None) => Err(error::from_backend(crate::BackendError::NotFound)),
            Err(err) => Err(err),
        }
    }

    /// Open a fully verified cached hit without contacting object storage.
    pub async fn acquire_cached(
        &self,
        artifact: &ArtifactRef,
    ) -> Result<PinnedArtifact, PlatformError> {
        self.try_hit(artifact)?
            .ok_or_else(|| error::from_backend(crate::BackendError::NotFound))
    }

    /// Evict unpinned entries from high watermark down to low watermark.
    pub async fn evict_if_needed(&self) -> Result<(), PlatformError> {
        self.evict_if_needed_except(None).await
    }

    async fn evict_if_needed_except(&self, keep: Option<&str>) -> Result<(), PlatformError> {
        let high = (self.config.max_bytes as f64 * self.config.high_watermark_ratio) as u64;
        let low = (self.config.max_bytes as f64 * self.config.low_watermark_ratio) as u64;
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| PlatformError::new(ErrorCode::PathInvalid, "cache lock poisoned"))?;
        if inner.total_bytes <= high {
            return Ok(());
        }
        let order: Vec<String> = inner.lru.iter().cloned().collect();
        for digest in order {
            if inner.total_bytes <= low {
                break;
            }
            if keep == Some(digest.as_str()) {
                continue;
            }
            let Some(meta) = inner.entries.get(&digest) else {
                continue;
            };
            if Arc::strong_count(&meta.pin) > 1 {
                continue;
            }
            let path = cache_path(&self.root, &digest);
            if !is_safe_evict_target(&path) {
                continue;
            }
            let size = meta.size;
            match fs::remove_file(&path) {
                Ok(()) => {}
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => continue,
            }
            inner.entries.remove(&digest);
            inner.lru.retain(|d| d != &digest);
            inner.total_bytes = inner.total_bytes.saturating_sub(size);
        }
        Ok(())
    }

    pub(super) fn try_hit(
        &self,
        artifact: &ArtifactRef,
    ) -> Result<Option<PinnedArtifact>, PlatformError> {
        let digest = artifact.sha256_hex();
        let path = cache_path(&self.root, &digest);
        let mut file = match open_entry_fd(&path) {
            Ok(file) => file,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => {
                return Err(PlatformError::new(
                    ErrorCode::CacheEntryCorrupt,
                    "cache entry could not be opened",
                ));
            }
        };
        let meta = file.metadata().map_err(|_| {
            PlatformError::new(
                ErrorCode::CacheEntryCorrupt,
                "cache entry could not be inspected",
            )
        })?;
        if !meta.file_type().is_file() {
            return Err(PlatformError::new(
                ErrorCode::CacheEntryCorrupt,
                "cache entry is not a regular file",
            ));
        }
        if meta.len() != artifact.size() {
            return Err(PlatformError::new(
                ErrorCode::CacheEntryCorrupt,
                "cache entry size mismatch",
            ));
        }
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| PlatformError::new(ErrorCode::PathInvalid, "cache lock poisoned"))?;
        let entry = inner.entries.entry(digest.clone()).or_insert(EntryMeta {
            size: artifact.size(),
            pin: Arc::new(()),
            verified: false,
        });
        let needs_hash = !entry.verified;
        let pin = Arc::clone(&entry.pin);
        drop(inner);
        if needs_hash {
            if let Err(err) = hash_fd(&mut file, artifact) {
                drop(pin);
                return Err(err);
            }
            let _ = file.rewind();
        }
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| PlatformError::new(ErrorCode::PathInvalid, "cache lock poisoned"))?;
        let Some(entry) = inner.entries.get_mut(&digest) else {
            return Err(PlatformError::new(
                ErrorCode::CacheEntryCorrupt,
                "cache entry was removed during verification",
            ));
        };
        if needs_hash {
            entry.verified = true;
        }
        entry.size = artifact.size();
        inner.lru.retain(|d| d != &digest);
        inner.lru.push_back(digest.clone());
        drop(inner);
        Ok(Some(PinnedArtifact { file, _pin: pin }))
    }

    async fn singleflight_fetch(
        &self,
        store: &ArtifactStore,
        artifact: &ArtifactRef,
    ) -> Result<ArtifactRef, PlatformError> {
        let digest = artifact.sha256_hex();
        let cell = {
            let mut map = self.inflight.lock().await;
            map.entry(digest.clone())
                .or_insert_with(|| Arc::new(OnceCell::new()))
                .clone()
        };
        let result = cell
            .get_or_init(|| async { self.fetch_once(store, artifact).await })
            .await
            .clone();
        self.inflight.lock().await.remove(&digest);
        result
    }

    async fn fetch_once(
        &self,
        store: &ArtifactStore,
        artifact: &ArtifactRef,
    ) -> Result<ArtifactRef, PlatformError> {
        self.stream_into_cache(store, artifact).await?;
        self.evict_if_needed_except(Some(&artifact.sha256_hex()))
            .await?;
        Ok(artifact.clone())
    }

    async fn stream_into_cache(
        &self,
        store: &ArtifactStore,
        artifact: &ArtifactRef,
    ) -> Result<(), PlatformError> {
        let dest = cache_path(&self.root, &artifact.sha256_hex());
        let sha_root = self.root.join("sha256");
        ensure_child_dir(&self.root, "sha256")?;
        ensure_child_dir(&sha_root, &artifact.sha256_hex()[..2])?;
        let mut nonce = [0_u8; 8];
        rand::rng().fill(&mut nonce);
        let partial = dest.with_file_name(format!(
            ".partial.{}.{}",
            self.startup_id,
            hex::encode(nonce)
        ));
        let mut guard = PartialGuard {
            path: partial.clone(),
            persist: false,
        };
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(FILE_MODE)
            .custom_flags(libc_nofollow())
            .open(&partial)
            .map_err(|_| {
                PlatformError::new(
                    ErrorCode::DiskHardLimit,
                    "failed to create cache partial file",
                )
            })?;
        store.download_verified(artifact, &mut file).await?;
        file.sync_all().map_err(|_| {
            PlatformError::new(
                ErrorCode::DiskHardLimit,
                "failed to fsync cache partial file",
            )
        })?;
        drop(file);
        fs::rename(&partial, &dest).map_err(|_| {
            PlatformError::new(ErrorCode::PathInvalid, "failed to publish cache entry")
        })?;
        fsync_dir(dest.parent().unwrap_or(&self.root))?;
        guard.persist = true;
        drop(guard);
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| PlatformError::new(ErrorCode::PathInvalid, "cache lock poisoned"))?;
        if let Some(old) = inner.entries.remove(&artifact.sha256_hex()) {
            inner.total_bytes = inner.total_bytes.saturating_sub(old.size);
            inner.lru.retain(|d| d != &artifact.sha256_hex());
        }
        inner.entries.insert(
            artifact.sha256_hex(),
            EntryMeta {
                size: artifact.size(),
                pin: Arc::new(()),
                verified: true,
            },
        );
        inner.lru.push_back(artifact.sha256_hex());
        inner.total_bytes = inner.total_bytes.saturating_add(artifact.size());
        Ok(())
    }

    fn quarantine(&self, artifact: &ArtifactRef) {
        let digest = artifact.sha256_hex();
        let path = cache_path(&self.root, &digest);
        let _ = fs::remove_file(&path);
        if let Ok(mut inner) = self.inner.lock() {
            if let Some(meta) = inner.entries.remove(&digest) {
                inner.total_bytes = inner.total_bytes.saturating_sub(meta.size);
            }
            inner.lru.retain(|d| d != &digest);
        }
    }

    /// Current tracked byte total.
    pub async fn total_bytes(&self) -> u64 {
        self.inner.lock().map_or(0, |g| g.total_bytes)
    }

    /// Number of indexed cache entries.
    #[must_use]
    pub fn entry_count(&self) -> u64 {
        self.inner.lock().map_or(0, |g| g.entries.len() as u64)
    }

    /// Hash existing entries without quarantine, LRU updates, or directory creation.
    pub fn sample_integrity(&self) -> Result<crate::inspect::CacheSample, PlatformError> {
        let inner = self
            .inner
            .lock()
            .map_err(|_| PlatformError::new(ErrorCode::PathInvalid, "cache lock poisoned"))?;
        let entries = inner.entries.len() as u64;
        let bytes = inner.total_bytes;
        let digests: Vec<(String, u64)> = inner
            .entries
            .iter()
            .map(|(d, m)| (d.clone(), m.size))
            .collect();
        drop(inner);
        let mut corrupt = false;
        for (digest, size) in digests.into_iter().take(32) {
            let path = cache_path(&self.root, &digest);
            let Ok(mut file) = open_entry_fd(&path) else {
                corrupt = true;
                continue;
            };
            let Ok(meta) = file.metadata() else {
                corrupt = true;
                continue;
            };
            if !meta.file_type().is_file() || meta.len() != size {
                corrupt = true;
                continue;
            }
            let Ok(artifact) =
                ArtifactRef::new(crate::artifact::ARTIFACT_KEY_VERSION, &digest, size)
            else {
                corrupt = true;
                continue;
            };
            if hash_fd(&mut file, &artifact).is_err() {
                corrupt = true;
            }
        }
        Ok(crate::inspect::CacheSample {
            entries,
            bytes,
            corrupt,
        })
    }
}
