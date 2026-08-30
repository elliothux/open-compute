//! Verified local artifact cache. S3 remains the authority.

use crate::artifact::{ArtifactRef, parse_sha256};
use crate::error::{self, S3Stage};
use crate::store::ArtifactStore;
use open_compute_core::{CacheConfig, ErrorCode, PlatformError, StartupId};
use rand::Rng;
use rustix::fd::{AsFd, OwnedFd};
use rustix::fs::{Mode, OFlags, fchmod, open, openat};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, VecDeque};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek};
#[cfg(target_os = "macos")]
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::{Duration, SystemTime};
use tokio::io::{AsyncRead, ReadBuf};
use tokio::sync::{Mutex as AsyncMutex, OnceCell};

const FILE_MODE: u32 = 0o600;

/// Readable cache handle that pins the entry against eviction.
#[derive(Debug)]
pub struct PinnedArtifact {
    file: File,
    _pin: Arc<()>,
}

/// Asynchronous reader that keeps its verified cache entry pinned until body completion.
#[derive(Debug)]
pub struct PinnedArtifactReader {
    file: tokio::fs::File,
    _pin: Arc<()>,
}

impl AsyncRead for PinnedArtifactReader {
    fn poll_read(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.file).poll_read(context, buffer)
    }
}

impl PinnedArtifact {
    /// Borrow the opened regular file.
    #[must_use]
    pub fn file(&self) -> &File {
        &self.file
    }

    /// Read the entire verified contents.
    pub fn read_all(&mut self) -> Result<Vec<u8>, PlatformError> {
        let mut buf = Vec::new();
        self.file.read_to_end(&mut buf).map_err(|_| {
            PlatformError::new(ErrorCode::PathInvalid, "failed to read cached artifact")
        })?;
        let _ = self.file.rewind();
        Ok(buf)
    }

    /// Convert into an async reader while preserving the eviction pin.
    #[must_use]
    pub fn into_async_reader(self) -> PinnedArtifactReader {
        PinnedArtifactReader {
            file: tokio::fs::File::from_std(self.file),
            _pin: self._pin,
        }
    }
}

#[derive(Debug)]
struct EntryMeta {
    size: u64,
    pin: Arc<()>,
    verified: bool,
}

#[derive(Debug)]
struct CacheInner {
    entries: HashMap<String, EntryMeta>,
    lru: VecDeque<String>,
    total_bytes: u64,
}

type InflightMap = HashMap<String, Arc<OnceCell<Result<ArtifactRef, PlatformError>>>>;

/// Local verified cache rooted at an explicit absolute directory.
#[derive(Debug)]
pub struct ArtifactCache {
    root: PathBuf,
    config: CacheConfig,
    startup_id: StartupId,
    inner: Arc<Mutex<CacheInner>>,
    inflight: AsyncMutex<InflightMap>,
}

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
            Ok(None) => Err(error::unavailable(S3Stage::NotFound)),
            Err(err) => Err(err),
        }
    }

    /// Open a fully verified cached hit without contacting S3.
    pub async fn acquire_cached(
        &self,
        artifact: &ArtifactRef,
    ) -> Result<PinnedArtifact, PlatformError> {
        self.try_hit(artifact)?
            .ok_or_else(|| error::unavailable(S3Stage::NotFound))
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

    fn try_hit(&self, artifact: &ArtifactRef) -> Result<Option<PinnedArtifact>, PlatformError> {
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
    pub(crate) fn sample_integrity(&self) -> Result<crate::inspect::CacheSample, PlatformError> {
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

struct PartialGuard {
    path: PathBuf,
    persist: bool,
}

impl Drop for PartialGuard {
    fn drop(&mut self) {
        if !self.persist {
            let _ = fs::remove_file(&self.path);
        }
    }
}

fn libc_nofollow() -> i32 {
    OFlags::NOFOLLOW.bits() as i32
}

fn cache_path(root: &Path, digest: &str) -> PathBuf {
    root.join("sha256").join(&digest[..2]).join(&digest[2..])
}

fn validate_cache_root(root: &Path) -> Result<(), PlatformError> {
    if !root.is_absolute() {
        return Err(PlatformError::new(
            ErrorCode::PathInvalid,
            "cache root must be an absolute path",
        ));
    }
    if root
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err(PlatformError::new(
            ErrorCode::PathInvalid,
            "cache root must not contain '..'",
        ));
    }
    Ok(())
}

fn path_invalid(msg: &'static str) -> PlatformError {
    PlatformError::new(ErrorCode::PathInvalid, msg)
}

#[cfg(target_os = "macos")]
fn is_macos_root_system_alias(parent: &OwnedFd, name: &std::ffi::OsStr, target: &[u8]) -> bool {
    let expected = match name.as_bytes() {
        b"tmp" => b"private/tmp".as_slice(),
        b"var" => b"private/var".as_slice(),
        _ => return false,
    };
    if target != expected {
        return false;
    }
    let Ok(root) = open(
        "/",
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
    ) else {
        return false;
    };
    let Ok(parent_stat) = rustix::fs::fstat(parent) else {
        return false;
    };
    let Ok(root_stat) = rustix::fs::fstat(&root) else {
        return false;
    };
    parent_stat.st_dev == root_stat.st_dev && parent_stat.st_ino == root_stat.st_ino
}

fn open_existing_component(
    parent: &OwnedFd,
    name: &std::ffi::OsStr,
) -> Result<OwnedFd, PlatformError> {
    match openat(
        parent,
        name,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    ) {
        Ok(child) => Ok(child),
        Err(err) if err == rustix::io::Errno::LOOP || err == rustix::io::Errno::NOTDIR => {
            let target = rustix::fs::readlinkat(parent, name, Vec::new())
                .map_err(|_| path_invalid("cache path must be a real directory"))?;
            #[cfg(not(target_os = "macos"))]
            {
                let _ = target;
                Err(path_invalid("cache path must be a real directory"))
            }
            #[cfg(target_os = "macos")]
            {
                if !is_macos_root_system_alias(parent, name, target.as_bytes()) {
                    return Err(path_invalid("cache path must be a real directory"));
                }
                openat(
                    parent,
                    name,
                    OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
                    Mode::empty(),
                )
                .map_err(|_| path_invalid("cache path must be a real directory"))
            }
        }
        Err(_) => Err(path_invalid("cache path must be a real directory")),
    }
}

fn open_dir_nofollow(path: &Path, create: bool) -> Result<OwnedFd, PlatformError> {
    let mut fd = open(
        "/",
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|_| path_invalid("failed to open cache directory"))?;
    let names: Vec<_> = path
        .components()
        .filter_map(|c| match c {
            std::path::Component::Normal(name) => Some(name),
            _ => None,
        })
        .collect();
    let last = names.len().saturating_sub(1);
    for (i, name) in names.into_iter().enumerate() {
        let is_last = i == last;
        match openat(
            &fd,
            name,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        ) {
            Ok(child) => fd = child,
            Err(err) if err == rustix::io::Errno::NOENT && create => {
                match rustix::fs::mkdirat(&fd, name, Mode::RWXU) {
                    Ok(()) => {}
                    Err(exist) if exist == rustix::io::Errno::EXIST => {}
                    Err(_) => return Err(path_invalid("failed to create cache directory")),
                }
                fd = openat(
                    &fd,
                    name,
                    OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                    Mode::empty(),
                )
                .map_err(|_| path_invalid("cache path must be a real directory"))?;
            }
            Err(err)
                if !is_last
                    && (err == rustix::io::Errno::LOOP || err == rustix::io::Errno::NOTDIR) =>
            {
                fd = open_existing_component(&fd, name)?;
            }
            Err(_) => {
                return Err(path_invalid("cache path must be a real directory"));
            }
        }
    }
    Ok(fd)
}

fn ensure_real_dir(path: &Path) -> Result<(), PlatformError> {
    let fd = open_dir_nofollow(path, true)?;
    fchmod(&fd, Mode::RWXU).map_err(|_| path_invalid("failed to set cache permissions"))
}

fn ensure_child_dir(parent: &Path, name: &str) -> Result<(), PlatformError> {
    let parent_fd = open_dir_nofollow(parent, false)?;
    match rustix::fs::mkdirat(&parent_fd, name, Mode::RWXU) {
        Ok(()) => {}
        Err(err) if err == rustix::io::Errno::EXIST => {}
        Err(_) => {
            return Err(path_invalid("failed to create cache directory"));
        }
    }
    let child = openat(
        &parent_fd,
        name,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|_| path_invalid("cache directory must not be a symlink"))?;
    fchmod(&child, Mode::RWXU).map_err(|_| path_invalid("failed to set cache permissions"))
}

fn fsync_dir(path: &Path) -> Result<(), PlatformError> {
    let dir = open(
        path,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|_| PlatformError::new(ErrorCode::PathInvalid, "failed to open cache directory"))?;
    rustix::fs::fsync(dir.as_fd())
        .map_err(|_| PlatformError::new(ErrorCode::PathInvalid, "failed to fsync cache directory"))
}

fn open_entry_fd(path: &Path) -> std::io::Result<File> {
    OpenOptions::new()
        .read(true)
        .custom_flags(libc_nofollow())
        .open(path)
}

fn hash_fd(file: &mut File, artifact: &ArtifactRef) -> Result<(), PlatformError> {
    #[cfg(test)]
    test_hooks::run_hash_pause();
    let mut hasher = Sha256::new();
    let mut buf = [0_u8; 8192];
    let mut total = 0_u64;
    loop {
        let n = file.read(&mut buf).map_err(|_| {
            PlatformError::new(
                ErrorCode::CacheEntryCorrupt,
                "cache entry could not be read",
            )
        })?;
        if n == 0 {
            break;
        }
        total += n as u64;
        hasher.update(&buf[..n]);
    }
    if total != artifact.size() {
        return Err(PlatformError::new(
            ErrorCode::CacheEntryCorrupt,
            "cache entry size mismatch",
        ));
    }
    if hasher.finalize().as_slice() != artifact.sha256_bytes() {
        return Err(PlatformError::new(
            ErrorCode::CacheEntryCorrupt,
            "cache entry digest mismatch",
        ));
    }
    Ok(())
}

fn is_safe_evict_target(path: &Path) -> bool {
    let Ok(meta) = fs::symlink_metadata(path) else {
        return false;
    };
    if meta.file_type().is_symlink() || !meta.file_type().is_file() {
        return false;
    }
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    !name.starts_with('.')
}

fn cleanup_stale_partials(sha_root: &Path, grace: Duration) {
    let now = SystemTime::now();
    let Ok(shards) = fs::read_dir(sha_root) else {
        return;
    };
    for shard in shards.flatten() {
        let path = shard.path();
        let Ok(meta) = fs::symlink_metadata(&path) else {
            continue;
        };
        if meta.file_type().is_symlink() || !meta.file_type().is_dir() {
            continue;
        }
        let Ok(ents) = fs::read_dir(&path) else {
            continue;
        };
        for ent in ents.flatten() {
            let p = ent.path();
            let name = ent.file_name();
            let name = name.to_string_lossy();
            if !name.starts_with(".partial.") {
                continue;
            }
            let Ok(meta) = fs::symlink_metadata(&p) else {
                continue;
            };
            if meta.file_type().is_symlink() {
                continue;
            }
            let Ok(modified) = meta.modified() else {
                continue;
            };
            if now.duration_since(modified).unwrap_or_default() > grace {
                let _ = fs::remove_file(&p);
            }
        }
    }
}

fn rebuild_index(sha_root: &Path) -> CacheInner {
    let mut inner = CacheInner {
        entries: HashMap::new(),
        lru: VecDeque::new(),
        total_bytes: 0,
    };
    let Ok(shards) = fs::read_dir(sha_root) else {
        return inner;
    };
    let mut found = Vec::new();
    for shard in shards.flatten() {
        let path = shard.path();
        let Ok(meta) = fs::symlink_metadata(&path) else {
            continue;
        };
        if meta.file_type().is_symlink() || !meta.file_type().is_dir() {
            continue;
        }
        let shard_name = shard.file_name();
        let shard_str = shard_name.to_string_lossy();
        if shard_str.len() != 2 {
            continue;
        }
        let Ok(ents) = fs::read_dir(&path) else {
            continue;
        };
        for ent in ents.flatten() {
            let name = ent.file_name();
            let rest = name.to_string_lossy();
            if rest.starts_with('.') || rest.len() != 62 {
                continue;
            }
            let digest = format!("{shard_str}{rest}");
            if parse_sha256(&digest).is_err() {
                continue;
            }
            let p = ent.path();
            let Ok(meta) = fs::symlink_metadata(&p) else {
                continue;
            };
            if meta.file_type().is_symlink() || !meta.file_type().is_file() {
                continue;
            }
            let mtime = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
            found.push((mtime, digest, meta.len()));
        }
    }
    found.sort_by_key(|(t, _, _)| *t);
    for (_, digest, size) in found {
        inner.total_bytes = inner.total_bytes.saturating_add(size);
        inner.entries.insert(
            digest.clone(),
            EntryMeta {
                size,
                pin: Arc::new(()),
                verified: false,
            },
        );
        inner.lru.push_back(digest);
    }
    inner
}

#[cfg(test)]
pub(crate) mod test_hooks {
    use super::{ArtifactCache, ArtifactRef, PinnedArtifact};
    use open_compute_core::PlatformError;
    use std::sync::{Arc, Mutex, OnceLock};

    type HashPauseFn = Arc<dyn Fn() + Send + Sync>;
    type HashPauseSlot = Mutex<Option<HashPauseFn>>;

    static HASH_PAUSE: OnceLock<HashPauseSlot> = OnceLock::new();

    fn hash_pause() -> &'static HashPauseSlot {
        HASH_PAUSE.get_or_init(|| Mutex::new(None))
    }

    pub(crate) struct HashPauseGuard;

    impl Drop for HashPauseGuard {
        fn drop(&mut self) {
            if let Ok(mut slot) = hash_pause().lock() {
                *slot = None;
            }
        }
    }

    pub(crate) fn install_hash_pause(hook: HashPauseFn) -> HashPauseGuard {
        *hash_pause().lock().expect("hash pause lock") = Some(hook);
        HashPauseGuard
    }

    pub(crate) fn run_hash_pause() {
        let hook = hash_pause().lock().ok().and_then(|g| g.clone());
        if let Some(hook) = hook {
            hook();
        }
    }

    impl ArtifactCache {
        pub(crate) fn try_hit_for_test(
            &self,
            artifact: &ArtifactRef,
        ) -> Result<Option<PinnedArtifact>, PlatformError> {
            self.try_hit(artifact)
        }

        pub(crate) fn is_indexed_for_test(&self, digest: &str) -> bool {
            self.inner
                .lock()
                .ok()
                .is_some_and(|g| g.entries.contains_key(digest))
        }
    }
}

#[cfg(test)]
#[path = "cache_tests.rs"]
mod coverage_tests;
