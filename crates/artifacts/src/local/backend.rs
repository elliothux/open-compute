use super::*;

impl std::fmt::Debug for LocalBackend {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LocalBackend")
            .field("prefix", &self.prefix)
            .field("r2_prefix", &self.r2_prefix)
            .field("authority_sha256", &hex::encode(self.authority_sha256))
            .field("max_object_bytes", &self.max_object_bytes)
            .finish_non_exhaustive()
    }
}

impl LocalBackend {
    pub(crate) fn inspect_authority(
        config: &LocalObjectStorageConfig,
    ) -> Result<(PlatformId, [u8; 32], u64), PlatformError> {
        let platform_id = Self::discover_platform_id(config)?;
        let root = open_local_root(&config.path, false)?;
        let marker: FormatMarker =
            read_json_bounded(&root, FORMAT_FILE, 64 * 1024).map_err(platform_integrity)?;
        let stat = rustix::fs::fstatvfs(&root).map_err(|_| platform_unavailable())?;
        let available = stat.f_bavail.saturating_mul(stat.f_frsize);
        Ok((platform_id, authority_sha256(&marker), available))
    }

    pub(crate) fn discover_platform_id(
        config: &LocalObjectStorageConfig,
    ) -> Result<PlatformId, PlatformError> {
        let root = open_local_root(&config.path, false)?;
        validate_dir(&root).map_err(platform_integrity)?;
        require_local_filesystem(&root)?;
        let marker: FormatMarker =
            read_json_bounded(&root, FORMAT_FILE, 64 * 1024).map_err(platform_integrity)?;
        if marker.schema_version != FORMAT_SCHEMA
            || marker.prefix != config.prefix
            || marker.r2_prefix != config.r2_prefix
            || !canonical_uuid_v7(&marker.root_id)
        {
            return Err(PlatformError::new(
                ErrorCode::ObjectStorageAuthorityMismatch,
                "local object authority marker does not match configuration",
            ));
        }
        PlatformId::from_str(&marker.platform_id).map_err(|_| {
            PlatformError::new(
                ErrorCode::ObjectStorageIntegrityError,
                "local object authority marker is invalid",
            )
        })
    }

    pub(crate) fn open(
        config: &LocalObjectStorageConfig,
        platform_id: PlatformId,
        max_object_bytes: u64,
    ) -> Result<Self, PlatformError> {
        if max_object_bytes == 0 {
            return Err(PlatformError::new(
                ErrorCode::LimitInvalid,
                "object storage maximum object size must be nonzero",
            ));
        }
        let root = Arc::new(open_local_root(&config.path, true)?);
        validate_dir(&root).map_err(platform_integrity)?;
        require_local_filesystem(&root)?;
        let lock_fd = openat(
            &root,
            LOCK_FILE,
            OFlags::RDWR | OFlags::CREATE | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::RUSR | Mode::WUSR,
        )
        .map_err(|_| platform_unavailable())?;
        validate_regular(&lock_fd, None).map_err(platform_integrity)?;
        flock(&lock_fd, FlockOperation::NonBlockingLockExclusive).map_err(|_| {
            PlatformError::new(
                ErrorCode::DataDirInUse,
                "local object authority is already owned by another process",
            )
        })?;
        let lock = Arc::new(File::from(lock_fd));
        let marker = load_or_initialize_marker(&root, config, platform_id)?;
        ensure_dir(&root, OBJECTS_DIR).map_err(platform_integrity)?;
        ensure_dir(&root, MULTIPART_DIR).map_err(platform_integrity)?;
        validate_root_entries(&root).map_err(platform_integrity)?;
        let authority_sha256 = authority_sha256(&marker);
        let backend = Self {
            root,
            _lock: lock,
            prefix: Arc::from(config.prefix.as_str()),
            r2_prefix: Arc::from(config.r2_prefix.as_str()),
            authority_sha256,
            max_object_bytes,
            free_space_hard_bytes: config.free_space_hard_bytes,
            partial_grace_ms: config.partial_grace_ms,
            key_locks: Arc::new((0..64).map(|_| Mutex::new(())).collect()),
            #[cfg(test)]
            fault: Arc::new(AtomicU8::new(0)),
        };
        Ok(backend)
    }

    #[cfg(test)]
    pub(crate) fn inject_fault(&self, point: LocalFaultPoint) {
        self.fault.store(point as u8, Ordering::SeqCst);
    }

    #[cfg(test)]
    fn trip_fault(&self, point: LocalFaultPoint) -> bool {
        self.fault
            .compare_exchange(point as u8, 0, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
    }

    pub(crate) async fn recover(&self) -> Result<(), BackendError> {
        let backend = self.clone();
        tokio::task::spawn_blocking(move || backend.recover_owned_partials())
            .await
            .map_err(|_| BackendError::Unavailable)?
    }

    pub(crate) fn prefix(&self) -> &str {
        &self.prefix
    }

    pub(crate) fn r2_prefix(&self) -> &str {
        &self.r2_prefix
    }

    pub(crate) const fn authority_sha256(&self) -> [u8; 32] {
        self.authority_sha256
    }

    pub(crate) const fn max_object_bytes(&self) -> u64 {
        self.max_object_bytes
    }

    pub(crate) fn available_bytes(&self) -> Result<u64, BackendError> {
        let stat = rustix::fs::fstatvfs(&self.root).map_err(|_| BackendError::Unavailable)?;
        Ok(stat.f_bavail.saturating_mul(stat.f_frsize))
    }

    pub(crate) async fn put(
        &self,
        key: &ObjectKey,
        source: ObjectSource,
        options: PutOptions,
    ) -> Result<ObjectMetadata, BackendError> {
        let lock_index = key_lock_index(key);
        let _guard = self.key_locks[lock_index].lock().await;
        let backend = self.clone();
        let key = key.clone();
        tokio::task::spawn_blocking(move || backend.put_sync(&key, source, options))
            .await
            .map_err(|_| BackendError::Unavailable)?
    }

    fn put_sync(
        &self,
        key: &ObjectKey,
        source: ObjectSource,
        options: PutOptions,
    ) -> Result<ObjectMetadata, BackendError> {
        if source.length() > self.max_object_bytes {
            return Err(BackendError::Capacity);
        }
        self.check_capacity(source.length())?;
        let parent = self.object_parent(key, true)?;
        let partial = format!(".partial-{}", uuid::Uuid::now_v7());
        let mut guard = PartialGuard::new(dup_fd(&parent)?, partial.clone());
        let partial_fd = create_regular(&parent, &partial)?;
        let mut file = File::from(partial_fd);
        let header = seal_source(
            &mut file,
            key,
            source,
            options.metadata,
            options.customer_key.as_ref(),
        )?;
        #[cfg(test)]
        if self.trip_fault(LocalFaultPoint::BeforeEnvelopeFsync) {
            guard.persist = true;
            return Err(BackendError::Unavailable);
        }
        file.sync_all().map_err(|_| BackendError::Unavailable)?;
        #[cfg(test)]
        if self.trip_fault(LocalFaultPoint::AfterEnvelopeFsync) {
            guard.persist = true;
            return Err(BackendError::Unavailable);
        }
        let current = read_optional_header(&parent, OBJECT_FILE, key)?;
        match &options.mode {
            PutMode::CreateOnly if current.is_some() => {
                return Err(BackendError::PreconditionFailed);
            }
            PutMode::IfMatch(expected)
                if current.as_ref().map(|value| &value.etag) != Some(expected) =>
            {
                return Err(BackendError::PreconditionFailed);
            }
            _ => {}
        }
        #[cfg(test)]
        if self.trip_fault(LocalFaultPoint::BeforePublishRename) {
            guard.persist = true;
            return Err(BackendError::Unavailable);
        }
        match options.mode {
            PutMode::CreateOnly => renameat_with(
                &parent,
                partial.as_str(),
                &parent,
                OBJECT_FILE,
                RenameFlags::NOREPLACE,
            )
            .map_err(|error| {
                if error == rustix::io::Errno::EXIST {
                    BackendError::PreconditionFailed
                } else {
                    BackendError::Unavailable
                }
            })?,
            PutMode::Replace | PutMode::IfMatch(_) => {
                renameat(&parent, partial.as_str(), &parent, OBJECT_FILE)
                    .map_err(|_| BackendError::Unavailable)?;
            }
        }
        guard.persist = true;
        #[cfg(test)]
        if self.trip_fault(LocalFaultPoint::AfterPublishRename) {
            return Err(BackendError::Unavailable);
        }
        fsync(parent.as_fd()).map_err(|_| BackendError::Unavailable)?;
        Ok(header.metadata)
    }

    pub(crate) async fn head(
        &self,
        key: &ObjectKey,
        options: HeadOptions,
    ) -> Result<ObjectMetadata, BackendError> {
        let backend = self.clone();
        let key = key.clone();
        tokio::task::spawn_blocking(move || {
            let parent = backend.object_parent(&key, false)?;
            let header = read_header(&parent, OBJECT_FILE, &key)?;
            verify_customer(&header, options.customer_key.as_ref())?;
            Ok(header.metadata)
        })
        .await
        .map_err(|_| BackendError::Unavailable)?
    }

    pub(crate) async fn get(
        &self,
        key: &ObjectKey,
        options: GetOptions,
    ) -> Result<ObjectGet, BackendError> {
        let backend = self.clone();
        let key = key.clone();
        let (file, header, customer, range) = tokio::task::spawn_blocking(move || {
            let parent = backend.object_parent(&key, false)?;
            let fd = open_regular(&parent, OBJECT_FILE)?;
            let mut file = File::from(fd);
            let header = read_header_from_file(&mut file, &key)?;
            verify_customer(&header, options.customer_key.as_ref())?;
            if options
                .if_match
                .as_ref()
                .is_some_and(|etag| etag != &header.etag)
            {
                return Err(BackendError::PreconditionFailed);
            }
            let range = match options.range {
                Some(range) if range.end < range.start || range.start >= header.size => {
                    return Err(BackendError::InvalidRange);
                }
                Some(range) => Some(ObjectRange {
                    start: range.start,
                    end: range.end.min(header.size.saturating_sub(1)),
                }),
                None => None,
            };
            Ok((file, header, options.customer_key, range))
        })
        .await
        .map_err(|_| BackendError::Unavailable)??;
        let (sender, receiver) = mpsc::channel(4);
        let header_for_stream = header.clone();
        tokio::task::spawn_blocking(move || {
            let result = stream_payload(file, header_for_stream, customer, range, &sender);
            if let Err(error) = result {
                let _ = sender.blocking_send(Err(std::io::Error::other(error.to_string())));
            }
        });
        Ok(ObjectGet {
            metadata: header.metadata,
            range,
            body: ObjectBody::from_local(receiver),
        })
    }

    pub(crate) async fn delete(&self, key: &ObjectKey) -> Result<(), BackendError> {
        let lock_index = key_lock_index(key);
        let _guard = self.key_locks[lock_index].lock().await;
        let backend = self.clone();
        let key = key.clone();
        tokio::task::spawn_blocking(move || {
            let parent = match backend.object_parent(&key, false) {
                Ok(parent) => parent,
                Err(BackendError::NotFound) => return Ok(()),
                Err(error) => return Err(error),
            };
            match unlinkat(&parent, OBJECT_FILE, AtFlags::empty()) {
                Ok(()) => {
                    #[cfg(test)]
                    if backend.trip_fault(LocalFaultPoint::AfterDeleteUnlink) {
                        return Err(BackendError::Unavailable);
                    }
                    fsync(parent.as_fd()).map_err(|_| BackendError::Unavailable)
                }
                Err(error) if error == rustix::io::Errno::NOENT => Ok(()),
                Err(_) => Err(BackendError::Unavailable),
            }
        })
        .await
        .map_err(|_| BackendError::Unavailable)?
    }

    pub(crate) async fn delete_many(&self, keys: &[ObjectKey]) -> Result<bool, BackendError> {
        for key in keys {
            self.delete(key).await?;
        }
        Ok(false)
    }

    pub(crate) async fn list(
        &self,
        prefix: &str,
        limit: u16,
        cursor: Option<&str>,
    ) -> Result<ListPage, BackendError> {
        if limit == 0 || prefix.len() > crate::backend::OBJECT_KEY_MAX_BYTES {
            return Err(BackendError::InvalidKey);
        }
        let backend = self.clone();
        let prefix = prefix.to_owned();
        let cursor = cursor.map(str::to_owned);
        tokio::task::spawn_blocking(move || backend.list_sync(&prefix, limit, cursor.as_deref()))
            .await
            .map_err(|_| BackendError::Unavailable)?
    }

    fn list_sync(
        &self,
        prefix: &str,
        limit: u16,
        cursor: Option<&str>,
    ) -> Result<ListPage, BackendError> {
        let cursor = cursor.map(decode_cursor).transpose()?;
        let objects = open_child_dir(&self.root, OBJECTS_DIR)?;
        let mut entries = Vec::new();
        let mut budget = ScanBudget::new();
        scan_objects(&objects, &[], &mut entries, &mut budget)?;
        entries.retain(|entry| {
            entry.key.as_str().starts_with(prefix)
                && cursor
                    .as_ref()
                    .is_none_or(|cursor| entry.key.as_str() > cursor.as_str())
        });
        entries.sort_by(|left, right| left.key.cmp(&right.key));
        let truncated = entries.len() > usize::from(limit);
        entries.truncate(usize::from(limit));
        let next_cursor = truncated
            .then(|| entries.last().map(|entry| encode_cursor(&entry.key)))
            .flatten();
        Ok(ListPage {
            objects: entries,
            next_cursor,
        })
    }

    pub(crate) async fn create_multipart(
        &self,
        key: &ObjectKey,
        metadata: ObjectMetadata,
        customer_key: Option<CustomerKey>,
    ) -> Result<String, BackendError> {
        let upload_id = uuid::Uuid::now_v7().hyphenated().to_string();
        let backend = self.clone();
        let key = key.clone();
        tokio::task::spawn_blocking(move || {
            let multipart = open_child_dir(&backend.root, MULTIPART_DIR)?;
            mkdirat(&multipart, upload_id.as_str(), Mode::RWXU)
                .map_err(|_| BackendError::Unavailable)?;
            let upload = open_child_dir(&multipart, &upload_id)?;
            ensure_dir(&upload, PARTS_DIR)?;
            let encryption = customer_key
                .as_ref()
                .map(|key_value| encryption_header(&key, key_value))
                .transpose()?;
            let manifest = MultipartManifest {
                schema_version: FORMAT_SCHEMA,
                upload_id: upload_id.clone(),
                key,
                metadata,
                encryption,
                created_at_ms: open_compute_core::wall_time_ms(),
                status: MultipartStatus::Uploading,
            };
            write_json_create(&upload, MANIFEST_FILE, &manifest)?;
            fsync(upload.as_fd()).map_err(|_| BackendError::Unavailable)?;
            fsync(multipart.as_fd()).map_err(|_| BackendError::Unavailable)?;
            Ok(upload_id)
        })
        .await
        .map_err(|_| BackendError::Unavailable)?
    }

    pub(crate) async fn upload_part(
        &self,
        key: &ObjectKey,
        upload_id: &str,
        part_number: i32,
        source: ObjectSource,
        customer_key: Option<CustomerKey>,
    ) -> Result<UploadedPart, BackendError> {
        if !(1..=10_000).contains(&part_number) || source.length() > self.max_object_bytes {
            return Err(BackendError::MultipartInvalid);
        }
        let lock_index = key_lock_index(key);
        let _guard = self.key_locks[lock_index].lock().await;
        let backend = self.clone();
        let key = key.clone();
        let upload_id = validate_upload_id(upload_id)?.to_owned();
        tokio::task::spawn_blocking(move || {
            let upload = backend.open_upload(&upload_id)?;
            let manifest = read_manifest(&upload)?;
            validate_manifest(&manifest, &key, &upload_id, customer_key.as_ref())?;
            if manifest.status != MultipartStatus::Uploading {
                return Err(BackendError::MultipartInvalid);
            }
            let parts = open_child_dir(&upload, PARTS_DIR)?;
            let final_name = format!("{part_number}.ocpart");
            let partial = format!(".partial-{}", uuid::Uuid::now_v7());
            let mut guard = PartialGuard::new(dup_fd(&parts)?, partial.clone());
            let fd = create_regular(&parts, &partial)?;
            let mut file = File::from(fd);
            let part_key = multipart_part_key(&key, &upload_id, part_number)?;
            let header = seal_source(
                &mut file,
                &part_key,
                source,
                ObjectMetadata::default(),
                customer_key.as_ref(),
            )?;
            file.sync_all().map_err(|_| BackendError::Unavailable)?;
            renameat(&parts, partial.as_str(), &parts, final_name.as_str())
                .map_err(|_| BackendError::Unavailable)?;
            guard.persist = true;
            fsync(parts.as_fd()).map_err(|_| BackendError::Unavailable)?;
            Ok(UploadedPart {
                part_number,
                etag: header.etag,
            })
        })
        .await
        .map_err(|_| BackendError::Unavailable)?
    }

    pub(crate) async fn list_multipart(
        &self,
        key: &ObjectKey,
    ) -> Result<Vec<String>, BackendError> {
        let lock_index = key_lock_index(key);
        let _guard = self.key_locks[lock_index].lock().await;
        let backend = self.clone();
        let key = key.clone();
        tokio::task::spawn_blocking(move || backend.list_multipart_sync(&key))
            .await
            .map_err(|_| BackendError::Unavailable)?
    }

    fn list_multipart_sync(&self, key: &ObjectKey) -> Result<Vec<String>, BackendError> {
        let multipart = open_child_dir(&self.root, MULTIPART_DIR)?;
        let mut ids = Vec::new();
        let mut budget = ScanBudget::new();
        for name in dir_names(&multipart)? {
            budget.charge(0)?;
            let Some(name) = name.to_str() else {
                return Err(BackendError::Corrupt);
            };
            validate_upload_id(name)?;
            let upload = open_child_dir(&multipart, name)?;
            let manifest_fd = open_regular(&upload, MANIFEST_FILE)?;
            let stat = fstat(&manifest_fd).map_err(|_| BackendError::Unavailable)?;
            budget.charge(stat.st_size as u64)?;
            let manifest = read_manifest(&upload)?;
            if manifest.status == MultipartStatus::Uploading && &manifest.key == key {
                ids.push(name.to_owned());
            }
        }
        ids.sort();
        Ok(ids)
    }

    pub(crate) async fn complete_multipart(
        &self,
        key: &ObjectKey,
        upload_id: &str,
        parts: &[UploadedPart],
        customer_key: Option<CustomerKey>,
    ) -> Result<ObjectMetadata, BackendError> {
        if parts.is_empty()
            || parts
                .windows(2)
                .any(|pair| pair[0].part_number >= pair[1].part_number)
        {
            return Err(BackendError::MultipartInvalid);
        }
        let lock_index = key_lock_index(key);
        let _guard = self.key_locks[lock_index].lock().await;
        let backend = self.clone();
        let key = key.clone();
        let upload_id = validate_upload_id(upload_id)?.to_owned();
        let parts = parts.to_vec();
        tokio::task::spawn_blocking(move || {
            let upload = backend.open_upload(&upload_id)?;
            let mut manifest = read_manifest(&upload)?;
            validate_manifest(&manifest, &key, &upload_id, customer_key.as_ref())?;
            if manifest.status == MultipartStatus::Aborting {
                return Err(BackendError::MultipartInvalid);
            }
            let parts_dir = open_child_dir(&upload, PARTS_DIR)?;
            let mut readers = VecDeque::new();
            let mut total = 0_u64;
            for requested in &parts {
                let name = format!("{}.ocpart", requested.part_number);
                let fd = open_regular(&parts_dir, &name)?;
                let mut file = File::from(fd);
                let part_key = multipart_part_key(&key, &upload_id, requested.part_number)?;
                let header = read_header_from_file(&mut file, &part_key)?;
                verify_customer(&header, customer_key.as_ref())?;
                if header.etag != requested.etag {
                    return Err(BackendError::MultipartInvalid);
                }
                total = total
                    .checked_add(header.size)
                    .ok_or(BackendError::Capacity)?;
                readers.push_back(PayloadReader::full(file, header, customer_key.clone())?);
            }
            if total > backend.max_object_bytes {
                return Err(BackendError::Capacity);
            }
            backend.check_capacity(total)?;
            let parent = backend.object_parent(&key, true)?;
            let partial = format!(".partial-{}", uuid::Uuid::now_v7());
            let mut partial_guard = PartialGuard::new(dup_fd(&parent)?, partial.clone());
            let fd = create_regular(&parent, &partial)?;
            let mut file = File::from(fd);
            let mut source = MultipartConcatReader { readers };
            let mut header = seal_reader(
                &mut file,
                &key,
                &mut source,
                total,
                manifest.metadata.clone(),
                customer_key.as_ref(),
            )?;
            if header.size != total {
                return Err(BackendError::Corrupt);
            }
            let etag = multipart_etag(&parts)?;
            header.etag.clone_from(&etag);
            header.metadata.etag = etag;
            write_header(&mut file, &header)?;
            file.sync_all().map_err(|_| BackendError::Unavailable)?;
            manifest.status = MultipartStatus::Publishing {
                etag: header.etag.clone(),
            };
            write_json_replace(&upload, MANIFEST_FILE, &manifest)?;
            #[cfg(test)]
            if backend.trip_fault(LocalFaultPoint::MultipartIntentCommitted) {
                partial_guard.persist = true;
                return Err(BackendError::Unavailable);
            }
            #[cfg(test)]
            if backend.trip_fault(LocalFaultPoint::MultipartBeforePublish) {
                partial_guard.persist = true;
                return Err(BackendError::Unavailable);
            }
            renameat(&parent, partial.as_str(), &parent, OBJECT_FILE)
                .map_err(|_| BackendError::Unavailable)?;
            partial_guard.persist = true;
            #[cfg(test)]
            if backend.trip_fault(LocalFaultPoint::MultipartAfterPublish) {
                return Err(BackendError::Unavailable);
            }
            fsync(parent.as_fd()).map_err(|_| BackendError::Unavailable)?;
            #[cfg(test)]
            if backend.trip_fault(LocalFaultPoint::MultipartBeforeRetire) {
                return Err(BackendError::Unavailable);
            }
            retire_upload_dir(&backend.root, &upload_id)?;
            Ok(header.metadata)
        })
        .await
        .map_err(|_| BackendError::Unavailable)?
    }

    pub(crate) async fn abort_multipart(
        &self,
        key: &ObjectKey,
        upload_id: &str,
    ) -> Result<(), BackendError> {
        let lock_index = key_lock_index(key);
        let _guard = self.key_locks[lock_index].lock().await;
        let backend = self.clone();
        let key = key.clone();
        let upload_id = validate_upload_id(upload_id)?.to_owned();
        tokio::task::spawn_blocking(move || {
            let upload = match backend.open_upload(&upload_id) {
                Ok(upload) => upload,
                Err(BackendError::NotFound) => return Ok(()),
                Err(error) => return Err(error),
            };
            let mut manifest = read_manifest(&upload)?;
            if manifest.key != key {
                return Err(BackendError::MultipartInvalid);
            }
            manifest.status = MultipartStatus::Aborting;
            write_json_replace(&upload, MANIFEST_FILE, &manifest)?;
            #[cfg(test)]
            if backend.trip_fault(LocalFaultPoint::MultipartAbortIntent) {
                return Err(BackendError::Unavailable);
            }
            retire_upload_dir(&backend.root, &upload_id)
        })
        .await
        .map_err(|_| BackendError::Unavailable)?
    }

    fn open_upload(&self, upload_id: &str) -> Result<OwnedFd, BackendError> {
        let multipart = open_child_dir(&self.root, MULTIPART_DIR)?;
        open_child_dir(&multipart, upload_id)
    }

    fn object_parent(&self, key: &ObjectKey, create: bool) -> Result<OwnedFd, BackendError> {
        let mut fd = open_child_dir(&self.root, OBJECTS_DIR)?;
        for segment in key.as_str().split('/') {
            if create {
                ensure_dir(&fd, segment)?;
            }
            fd = open_child_dir(&fd, segment)?;
        }
        Ok(fd)
    }

    fn check_capacity(&self, additional: u64) -> Result<(), BackendError> {
        let stat = rustix::fs::fstatvfs(&self.root).map_err(|_| BackendError::Unavailable)?;
        let available = stat.f_bavail.saturating_mul(stat.f_frsize);
        if available < self.free_space_hard_bytes.saturating_add(additional) {
            return Err(BackendError::Capacity);
        }
        Ok(())
    }

    fn recover_owned_partials(&self) -> Result<(), BackendError> {
        let objects = open_child_dir(&self.root, OBJECTS_DIR)?;
        let mut budget = ScanBudget::new();
        let cutoff_ms =
            open_compute_core::wall_time_ms().saturating_sub(self.partial_grace_ms as i64);
        remove_object_partials(&objects, &mut budget, cutoff_ms)?;
        self.reconcile_multipart(&mut budget, cutoff_ms)?;
        Ok(())
    }

    fn reconcile_multipart(
        &self,
        budget: &mut ScanBudget,
        cutoff_ms: i64,
    ) -> Result<(), BackendError> {
        let multipart = open_child_dir(&self.root, MULTIPART_DIR)?;
        for name in dir_names(&multipart)? {
            budget.charge(0)?;
            let Some(name) = name.to_str() else {
                return Err(BackendError::Corrupt);
            };
            if let Some(upload_id) = name.strip_prefix(".gc-") {
                validate_upload_id(upload_id)?;
                remove_retired_upload(&multipart, name)?;
                continue;
            }
            validate_upload_id(name)?;
            let upload = open_child_dir(&multipart, name)?;
            let names = dir_names(&upload)?;
            if !names.iter().any(|entry| entry == OsStr::new(MANIFEST_FILE))
                || !names.iter().any(|entry| entry == OsStr::new(PARTS_DIR))
            {
                return Err(BackendError::Corrupt);
            }
            for entry in &names {
                budget.charge(0)?;
                let Some(entry) = entry.to_str() else {
                    return Err(BackendError::Corrupt);
                };
                if entry != MANIFEST_FILE
                    && entry != PARTS_DIR
                    && validate_partial_name(entry).is_err()
                {
                    return Err(BackendError::Corrupt);
                }
                if entry.starts_with(".partial-") {
                    remove_stale_partial(&upload, entry, cutoff_ms, budget)?;
                }
            }
            let manifest = read_manifest(&upload)?;
            validate_manifest_record(&manifest, name)?;
            let parts = open_child_dir(&upload, PARTS_DIR)?;
            for part_name in dir_names(&parts)? {
                let Some(part_name) = part_name.to_str() else {
                    return Err(BackendError::Corrupt);
                };
                if part_name.starts_with(".partial-") {
                    validate_partial_name(part_name)?;
                    remove_stale_partial(&parts, part_name, cutoff_ms, budget)?;
                    continue;
                }
                let part_number = parse_part_name(part_name)?;
                let fd = open_regular(&parts, part_name)?;
                let stat = fstat(&fd).map_err(|_| BackendError::Unavailable)?;
                budget.charge((stat.st_size as u64).min(HEADER_BYTES as u64))?;
                let mut file = File::from(fd);
                let part_key = multipart_part_key(&manifest.key, name, part_number)?;
                let _ = read_header_from_file(&mut file, &part_key)?;
            }
            let retire = match &manifest.status {
                MultipartStatus::Uploading => false,
                MultipartStatus::Aborting => true,
                MultipartStatus::Publishing { etag } => {
                    match self.object_parent(&manifest.key, false) {
                        Ok(parent) => read_optional_header(&parent, OBJECT_FILE, &manifest.key)?
                            .is_some_and(|header| header.etag == *etag),
                        Err(BackendError::NotFound) => false,
                        Err(error) => return Err(error),
                    }
                }
            };
            if retire {
                retire_upload_dir(&self.root, name)?;
            }
        }
        fsync(multipart.as_fd()).map_err(|_| BackendError::Unavailable)
    }
}
