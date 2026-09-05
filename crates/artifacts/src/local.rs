//! Secure fd-relative local object authority.

use crate::backend::{
    BackendError, CustomerKey, GetOptions, HeadOptions, ListPage, ListedObject, ObjectBody,
    ObjectGet, ObjectKey, ObjectMetadata, ObjectRange, ObjectSource, PutMode, PutOptions,
    UploadedPart,
};
use base64::Engine as _;
use bytes::Bytes;
use chacha20poly1305::aead::{Aead as _, KeyInit as _, Payload};
use chacha20poly1305::{Key, XChaCha20Poly1305, XNonce};
use md5::{Digest as _, Md5};
use open_compute_core::{ErrorCode, LocalObjectStorageConfig, PlatformError, PlatformId};
use rand::RngCore as _;
use rustix::fd::{AsFd as _, OwnedFd};
use rustix::fs::{
    AtFlags, FlockOperation, Mode, OFlags, RenameFlags, fchmod, flock, fstat, fsync, mkdirat, open,
    openat, renameat, renameat_with, statat, unlinkat,
};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::collections::VecDeque;
use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io::{Read, Seek as _, SeekFrom, Write as _};
use std::os::unix::ffi::{OsStrExt as _, OsStringExt as _};
use std::str::FromStr as _;
use std::sync::Arc;
#[cfg(test)]
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::{Mutex, mpsc};

const FORMAT_SCHEMA: u32 = 1;
const HEADER_BYTES: usize = 64 * 1024;
const MAGIC: &[u8; 8] = b"OCOBJ001";
const CHUNK_BYTES: usize = 64 * 1024;
const AEAD_TAG_BYTES: usize = 16;
const MAX_SCAN_ENTRIES: usize = 1_000_000;
const MAX_SCAN_BYTES: u64 = 1 << 40;
const MAX_SCAN_DURATION: Duration = Duration::from_secs(30);
const OBJECT_FILE: &str = "object.ocobj";
const FORMAT_FILE: &str = "format.json";
const LOCK_FILE: &str = "backend.lock";
const OBJECTS_DIR: &str = "objects";
const MULTIPART_DIR: &str = "multipart";
const MANIFEST_FILE: &str = "manifest.json";
const PARTS_DIR: &str = "parts";
const CURSOR_PREFIX: &str = "local-v1:";

#[cfg(test)]
#[derive(Clone, Copy, Debug)]
#[repr(u8)]
pub(crate) enum LocalFaultPoint {
    BeforeEnvelopeFsync = 1,
    AfterEnvelopeFsync = 2,
    BeforePublishRename = 3,
    AfterPublishRename = 4,
    AfterDeleteUnlink = 5,
    MultipartIntentCommitted = 6,
    MultipartBeforePublish = 7,
    MultipartAfterPublish = 8,
    MultipartBeforeRetire = 9,
    MultipartAbortIntent = 10,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FormatMarker {
    schema_version: u32,
    platform_id: String,
    root_id: String,
    prefix: String,
    r2_prefix: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct EnvelopeHeader {
    schema_version: u32,
    key_sha256: String,
    size: u64,
    stored_size: u64,
    etag: String,
    last_modified_ms: i64,
    payload_sha256: String,
    metadata: ObjectMetadata,
    encryption: Option<EncryptionHeader>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct EncryptionHeader {
    algorithm: String,
    chunk_size: u32,
    object_version: String,
    nonce: String,
    verifier: String,
    ssec_key_md5: String,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct HeaderRecord {
    header: EnvelopeHeader,
    header_sha256: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct MultipartManifest {
    schema_version: u32,
    upload_id: String,
    key: ObjectKey,
    metadata: ObjectMetadata,
    encryption: Option<EncryptionHeader>,
    created_at_ms: i64,
    status: MultipartStatus,
}

struct ScanBudget {
    entries: usize,
    bytes: u64,
    started: Instant,
}

impl ScanBudget {
    fn new() -> Self {
        Self {
            entries: 0,
            bytes: 0,
            started: Instant::now(),
        }
    }

    fn charge(&mut self, bytes: u64) -> Result<(), BackendError> {
        self.entries = self.entries.saturating_add(1);
        self.bytes = self.bytes.saturating_add(bytes);
        if self.entries > MAX_SCAN_ENTRIES
            || self.bytes > MAX_SCAN_BYTES
            || self.started.elapsed() > MAX_SCAN_DURATION
        {
            return Err(BackendError::Capacity);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum MultipartStatus {
    Uploading,
    Publishing { etag: String },
    Aborting,
}

/// One exclusively locked local object authority.
#[derive(Clone)]
pub(crate) struct LocalBackend {
    root: Arc<OwnedFd>,
    _lock: Arc<File>,
    prefix: Arc<str>,
    r2_prefix: Arc<str>,
    authority_sha256: [u8; 32],
    max_object_bytes: u64,
    free_space_hard_bytes: u64,
    partial_grace_ms: u64,
    key_locks: Arc<Vec<Mutex<()>>>,
    #[cfg(test)]
    fault: Arc<AtomicU8>,
}

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
                created_at_ms: now_ms(),
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
        let cutoff_ms = now_ms().saturating_sub(self.partial_grace_ms as i64);
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

fn seal_source(
    file: &mut File,
    key: &ObjectKey,
    source: ObjectSource,
    metadata: ObjectMetadata,
    customer_key: Option<&CustomerKey>,
) -> Result<EnvelopeHeader, BackendError> {
    match source {
        ObjectSource::Bytes(bytes) => {
            let length = bytes.len() as u64;
            seal_reader(
                file,
                key,
                std::io::Cursor::new(bytes),
                length,
                metadata,
                customer_key,
            )
        }
        ObjectSource::File {
            file: mut source_file,
            length,
        } => {
            let metadata_on_disk = source_file
                .metadata()
                .map_err(|_| BackendError::Unavailable)?;
            if !metadata_on_disk.file_type().is_file()
                || metadata_on_disk.len() != length
                || metadata_on_disk.nlink() != 1
                || std::os::unix::fs::MetadataExt::mode(&metadata_on_disk) & 0o077 != 0
                || std::os::unix::fs::MetadataExt::uid(&metadata_on_disk)
                    != rustix::process::getuid().as_raw()
            {
                return Err(BackendError::Corrupt);
            }
            source_file
                .seek(SeekFrom::Start(0))
                .map_err(|_| BackendError::Unavailable)?;
            seal_reader(file, key, source_file, length, metadata, customer_key)
        }
    }
}

fn seal_reader(
    file: &mut File,
    key: &ObjectKey,
    mut source: impl Read,
    expected_size: u64,
    mut metadata: ObjectMetadata,
    customer_key: Option<&CustomerKey>,
) -> Result<EnvelopeHeader, BackendError> {
    file.write_all(&vec![0_u8; HEADER_BYTES])
        .map_err(|_| BackendError::Unavailable)?;
    let mut plaintext_sha256 = Sha256::new();
    let mut plaintext_md5 = Md5::new();
    let mut total = 0_u64;
    let mut stored = 0_u64;
    let mut buffer = vec![0_u8; CHUNK_BYTES];
    let encryption = customer_key
        .as_ref()
        .map(|key_value| encryption_header(key, key_value))
        .transpose()?;
    let cipher = customer_key
        .as_ref()
        .map(|value| XChaCha20Poly1305::new(Key::from_slice(value.bytes())));
    let nonce_base = encryption
        .as_ref()
        .map(|value| decode_nonce(&value.nonce))
        .transpose()?;
    let mut chunk_index = 0_u64;
    loop {
        let read = read_chunk(&mut source, &mut buffer)?;
        if read == 0 {
            break;
        }
        total = total
            .checked_add(read as u64)
            .ok_or(BackendError::Capacity)?;
        plaintext_sha256.update(&buffer[..read]);
        plaintext_md5.update(&buffer[..read]);
        if let (Some(cipher), Some(nonce), Some(encryption)) =
            (&cipher, nonce_base, encryption.as_ref())
        {
            let nonce = chunk_nonce(nonce, chunk_index);
            let ciphertext = cipher
                .encrypt(
                    XNonce::from_slice(&nonce),
                    Payload {
                        msg: &buffer[..read],
                        aad: &chunk_aad_from_digest(
                            &hex::encode(Sha256::digest(key.as_str().as_bytes())),
                            &encryption.object_version,
                            expected_size,
                            chunk_index,
                        ),
                    },
                )
                .map_err(|_| BackendError::Corrupt)?;
            file.write_all(&ciphertext)
                .map_err(|_| BackendError::Unavailable)?;
            stored = stored
                .checked_add(ciphertext.len() as u64)
                .ok_or(BackendError::Capacity)?;
        } else {
            file.write_all(&buffer[..read])
                .map_err(|_| BackendError::Unavailable)?;
            stored = stored
                .checked_add(read as u64)
                .ok_or(BackendError::Capacity)?;
        }
        chunk_index = chunk_index.checked_add(1).ok_or(BackendError::Capacity)?;
    }
    if total != expected_size {
        return Err(BackendError::Corrupt);
    }
    metadata.size = total;
    metadata.last_modified_ms = now_ms();
    let payload_sha256 = hex::encode(plaintext_sha256.finalize());
    metadata.etag = hex::encode(plaintext_md5.finalize());
    metadata.ssec_key_md5 = encryption.as_ref().map(|value| value.ssec_key_md5.clone());
    let header = EnvelopeHeader {
        schema_version: FORMAT_SCHEMA,
        key_sha256: hex::encode(Sha256::digest(key.as_str().as_bytes())),
        size: total,
        stored_size: stored,
        etag: metadata.etag.clone(),
        last_modified_ms: metadata.last_modified_ms,
        payload_sha256,
        metadata,
        encryption,
    };
    write_header(file, &header)?;
    Ok(header)
}

fn write_header(file: &mut File, header: &EnvelopeHeader) -> Result<(), BackendError> {
    let canonical = serde_json::to_vec(header).map_err(|_| BackendError::Corrupt)?;
    let record = HeaderRecord {
        header: header.clone(),
        header_sha256: hex::encode(Sha256::digest(&canonical)),
    };
    let bytes = serde_json::to_vec(&record).map_err(|_| BackendError::Corrupt)?;
    if bytes.len().saturating_add(12) > HEADER_BYTES {
        return Err(BackendError::Corrupt);
    }
    file.seek(SeekFrom::Start(0))
        .map_err(|_| BackendError::Unavailable)?;
    file.write_all(MAGIC)
        .and_then(|()| file.write_all(&(bytes.len() as u32).to_be_bytes()))
        .and_then(|()| file.write_all(&bytes))
        .map_err(|_| BackendError::Unavailable)
}

fn read_header_from_file(file: &mut File, key: &ObjectKey) -> Result<EnvelopeHeader, BackendError> {
    let metadata = file.metadata().map_err(|_| BackendError::Unavailable)?;
    if !metadata.file_type().is_file() || metadata.nlink() != 1 {
        return Err(BackendError::Corrupt);
    }
    let mut prefix = [0_u8; 12];
    file.seek(SeekFrom::Start(0))
        .and_then(|_| file.read_exact(&mut prefix))
        .map_err(|_| BackendError::Corrupt)?;
    if &prefix[..8] != MAGIC {
        return Err(BackendError::Corrupt);
    }
    let length = u32::from_be_bytes(
        prefix[8..12]
            .try_into()
            .map_err(|_| BackendError::Corrupt)?,
    ) as usize;
    if length == 0 || length.saturating_add(12) > HEADER_BYTES {
        return Err(BackendError::Corrupt);
    }
    let mut bytes = vec![0_u8; length];
    file.read_exact(&mut bytes)
        .map_err(|_| BackendError::Corrupt)?;
    let record: HeaderRecord = serde_json::from_slice(&bytes).map_err(|_| BackendError::Corrupt)?;
    let canonical = serde_json::to_vec(&record.header).map_err(|_| BackendError::Corrupt)?;
    if record.header_sha256 != hex::encode(Sha256::digest(canonical))
        || record.header.schema_version != FORMAT_SCHEMA
        || record.header.key_sha256 != hex::encode(Sha256::digest(key.as_str().as_bytes()))
        || record.header.etag != record.header.metadata.etag
        || record.header.size != record.header.metadata.size
        || record.header.last_modified_ms != record.header.metadata.last_modified_ms
        || metadata.len() != HEADER_BYTES as u64 + record.header.stored_size
        || !valid_envelope_header(&record.header)
    {
        return Err(BackendError::Corrupt);
    }
    Ok(record.header)
}

fn valid_envelope_header(header: &EnvelopeHeader) -> bool {
    let valid_sha256 = |value: &str| {
        value.len() == 64
            && value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    };
    if !valid_sha256(&header.key_sha256)
        || !valid_sha256(&header.payload_sha256)
        || !valid_etag(&header.etag)
        || header.last_modified_ms < 0
        || header.metadata.ssec_key_md5
            != header
                .encryption
                .as_ref()
                .map(|encryption| encryption.ssec_key_md5.clone())
    {
        return false;
    }
    match &header.encryption {
        None => header.stored_size == header.size,
        Some(encryption) => {
            let chunks = header.size.div_ceil(CHUNK_BYTES as u64);
            let expected = header
                .size
                .checked_add(chunks.saturating_mul(AEAD_TAG_BYTES as u64));
            expected == Some(header.stored_size)
                && encryption.algorithm == "xchacha20poly1305-chunked-v1"
                && encryption.chunk_size == CHUNK_BYTES as u32
                && canonical_uuid_v7(&encryption.object_version)
                && decode_nonce(&encryption.nonce).is_ok()
                && base64::engine::general_purpose::URL_SAFE_NO_PAD
                    .decode(&encryption.verifier)
                    .is_ok_and(|bytes| bytes.len() == AEAD_TAG_BYTES)
                && valid_lower_hex(&encryption.ssec_key_md5, 32)
        }
    }
}

fn read_header(
    parent: &OwnedFd,
    name: &str,
    key: &ObjectKey,
) -> Result<EnvelopeHeader, BackendError> {
    let fd = open_regular(parent, name)?;
    read_header_from_file(&mut File::from(fd), key)
}

fn read_optional_header(
    parent: &OwnedFd,
    name: &str,
    key: &ObjectKey,
) -> Result<Option<EnvelopeHeader>, BackendError> {
    match read_header(parent, name, key) {
        Ok(header) => Ok(Some(header)),
        Err(BackendError::NotFound) => Ok(None),
        Err(error) => Err(error),
    }
}

fn encryption_header(
    key: &ObjectKey,
    customer_key: &CustomerKey,
) -> Result<EncryptionHeader, BackendError> {
    let mut nonce = [0_u8; 24];
    rand::rng().fill_bytes(&mut nonce);
    let object_version = uuid::Uuid::now_v7().hyphenated().to_string();
    let cipher = XChaCha20Poly1305::new(Key::from_slice(customer_key.bytes()));
    let verifier_nonce = chunk_nonce(nonce, u64::MAX);
    let verifier = cipher
        .encrypt(
            XNonce::from_slice(&verifier_nonce),
            Payload {
                msg: &[],
                aad: &verifier_aad(key, &object_version),
            },
        )
        .map_err(|_| BackendError::Corrupt)?;
    Ok(EncryptionHeader {
        algorithm: "xchacha20poly1305-chunked-v1".to_owned(),
        chunk_size: CHUNK_BYTES as u32,
        object_version,
        nonce: base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(nonce),
        verifier: base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(verifier),
        ssec_key_md5: hex::encode(Md5::digest(customer_key.bytes())),
    })
}

fn verify_customer(
    header: &EnvelopeHeader,
    customer_key: Option<&CustomerKey>,
) -> Result<(), BackendError> {
    match (&header.encryption, customer_key) {
        (None, None) => Ok(()),
        (None, Some(_)) | (Some(_), None) => Err(BackendError::CustomerKeyInvalid),
        (Some(encryption), Some(customer)) => {
            if encryption.algorithm != "xchacha20poly1305-chunked-v1"
                || encryption.chunk_size != CHUNK_BYTES as u32
                || !canonical_uuid_v7(&encryption.object_version)
            {
                return Err(BackendError::Corrupt);
            }
            let nonce = decode_nonce(&encryption.nonce)?;
            let verifier = base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(&encryption.verifier)
                .map_err(|_| BackendError::Corrupt)?;
            let cipher = XChaCha20Poly1305::new(Key::from_slice(customer.bytes()));
            let actual = cipher
                .encrypt(
                    XNonce::from_slice(&chunk_nonce(nonce, u64::MAX)),
                    Payload {
                        msg: &[],
                        aad: &verifier_aad_from_digest(
                            &header.key_sha256,
                            &encryption.object_version,
                        ),
                    },
                )
                .map_err(|_| BackendError::CustomerKeyInvalid)?;
            if actual != verifier {
                return Err(BackendError::CustomerKeyInvalid);
            }
            Ok(())
        }
    }
}

fn stream_payload(
    file: File,
    header: EnvelopeHeader,
    customer_key: Option<CustomerKey>,
    range: Option<ObjectRange>,
    sender: &mpsc::Sender<Result<Bytes, std::io::Error>>,
) -> Result<(), BackendError> {
    let mut reader = match range {
        Some(range) => PayloadReader::range(file, header, customer_key, range)?,
        None => PayloadReader::full(file, header, customer_key)?,
    };
    let mut buffer = vec![0_u8; CHUNK_BYTES];
    loop {
        let read = reader
            .read(&mut buffer)
            .map_err(|_| BackendError::Corrupt)?;
        if read == 0 {
            break;
        }
        sender
            .blocking_send(Ok(Bytes::copy_from_slice(&buffer[..read])))
            .map_err(|_| BackendError::Unavailable)?;
    }
    Ok(())
}

struct PayloadReader {
    file: File,
    header: EnvelopeHeader,
    customer_key: Option<CustomerKey>,
    next_chunk: u64,
    current: Vec<u8>,
    current_offset: usize,
    remaining: u64,
    verify_full: bool,
    hasher: Sha256,
}

impl PayloadReader {
    fn full(
        mut file: File,
        header: EnvelopeHeader,
        customer_key: Option<CustomerKey>,
    ) -> Result<Self, BackendError> {
        file.seek(SeekFrom::Start(HEADER_BYTES as u64))
            .map_err(|_| BackendError::Corrupt)?;
        Ok(Self {
            remaining: header.size,
            file,
            header,
            customer_key,
            next_chunk: 0,
            current: Vec::new(),
            current_offset: 0,
            verify_full: true,
            hasher: Sha256::new(),
        })
    }

    fn range(
        mut file: File,
        header: EnvelopeHeader,
        customer_key: Option<CustomerKey>,
        range: ObjectRange,
    ) -> Result<Self, BackendError> {
        let first_chunk = range.start / CHUNK_BYTES as u64;
        let within = (range.start % CHUNK_BYTES as u64) as usize;
        if header.encryption.is_none() {
            file.seek(SeekFrom::Start(HEADER_BYTES as u64))
                .map_err(|_| BackendError::Corrupt)?;
            let mut remaining = header.size;
            let mut buffer = vec![0_u8; CHUNK_BYTES];
            let mut digest = Sha256::new();
            while remaining != 0 {
                let requested = buffer.len().min(remaining as usize);
                let count = file
                    .read(&mut buffer[..requested])
                    .map_err(|_| BackendError::Corrupt)?;
                if count == 0 {
                    return Err(BackendError::Corrupt);
                }
                digest.update(&buffer[..count]);
                remaining -= count as u64;
            }
            if hex::encode(digest.finalize()) != header.payload_sha256 {
                return Err(BackendError::Corrupt);
            }
        }
        let offset = if header.encryption.is_some() {
            HEADER_BYTES as u64 + first_chunk * (CHUNK_BYTES as u64 + AEAD_TAG_BYTES as u64)
        } else {
            HEADER_BYTES as u64 + range.start
        };
        file.seek(SeekFrom::Start(offset))
            .map_err(|_| BackendError::Corrupt)?;
        let mut reader = Self {
            remaining: range.end - range.start + 1,
            file,
            header,
            customer_key,
            next_chunk: first_chunk,
            current: Vec::new(),
            current_offset: 0,
            verify_full: false,
            hasher: Sha256::new(),
        };
        if reader.header.encryption.is_some() {
            reader.load_chunk().map_err(|_| BackendError::Corrupt)?;
            reader.current_offset = within;
        }
        Ok(reader)
    }

    fn load_chunk(&mut self) -> std::io::Result<()> {
        if self.header.encryption.is_none() {
            return Ok(());
        }
        let chunk_start = self.next_chunk.saturating_mul(CHUNK_BYTES as u64);
        if chunk_start >= self.header.size {
            self.current.clear();
            self.current_offset = 0;
            return Ok(());
        }
        let plain_len = (self.header.size - chunk_start).min(CHUNK_BYTES as u64) as usize;
        let mut ciphertext = vec![0_u8; plain_len + AEAD_TAG_BYTES];
        self.file.read_exact(&mut ciphertext)?;
        let encryption = self
            .header
            .encryption
            .as_ref()
            .ok_or_else(|| std::io::Error::other("missing local object encryption metadata"))?;
        let nonce = decode_nonce(&encryption.nonce)
            .map_err(|_| std::io::Error::other("invalid local object nonce"))?;
        let customer = self
            .customer_key
            .as_ref()
            .ok_or_else(|| std::io::Error::other("missing local object customer key"))?;
        let cipher = XChaCha20Poly1305::new(Key::from_slice(customer.bytes()));
        self.current = cipher
            .decrypt(
                XNonce::from_slice(&chunk_nonce(nonce, self.next_chunk)),
                Payload {
                    msg: &ciphertext,
                    aad: &chunk_aad_from_digest(
                        &self.header.key_sha256,
                        &encryption.object_version,
                        self.header.size,
                        self.next_chunk,
                    ),
                },
            )
            .map_err(|_| std::io::Error::other("local object authentication failed"))?;
        self.current_offset = 0;
        self.next_chunk += 1;
        Ok(())
    }
}

impl Read for PayloadReader {
    fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
        if self.remaining == 0 {
            if self.verify_full {
                let actual = std::mem::replace(&mut self.hasher, Sha256::new()).finalize();
                if hex::encode(actual) != self.header.payload_sha256 {
                    return Err(std::io::Error::other("local object checksum failed"));
                }
                self.verify_full = false;
            }
            return Ok(0);
        }
        if self.header.encryption.is_none() {
            let count = output.len().min(self.remaining as usize);
            let read = self.file.read(&mut output[..count])?;
            if read == 0 {
                return Err(std::io::Error::other("local object is truncated"));
            }
            self.remaining -= read as u64;
            if self.verify_full {
                self.hasher.update(&output[..read]);
            }
            return Ok(read);
        }
        if self.current_offset == self.current.len() {
            self.load_chunk()?;
        }
        let available = self.current.len().saturating_sub(self.current_offset);
        let count = output.len().min(available).min(self.remaining as usize);
        if count == 0 {
            return Err(std::io::Error::other("local object is truncated"));
        }
        output[..count]
            .copy_from_slice(&self.current[self.current_offset..self.current_offset + count]);
        self.current_offset += count;
        self.remaining -= count as u64;
        if self.verify_full {
            self.hasher.update(&output[..count]);
        }
        Ok(count)
    }
}

struct MultipartConcatReader {
    readers: VecDeque<PayloadReader>,
}

impl Read for MultipartConcatReader {
    fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
        loop {
            let Some(reader) = self.readers.front_mut() else {
                return Ok(0);
            };
            let count = reader.read(output)?;
            if count != 0 {
                return Ok(count);
            }
            self.readers.pop_front();
        }
    }
}

fn open_local_root(path: &std::path::Path, create: bool) -> Result<OwnedFd, PlatformError> {
    if !path.is_absolute() {
        return Err(platform_integrity(BackendError::Corrupt));
    }
    let names = path
        .components()
        .filter_map(|component| match component {
            std::path::Component::Normal(name) => Some(name.to_os_string()),
            _ => None,
        })
        .collect::<Vec<_>>();
    let mut fd = open(
        "/",
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|_| platform_unavailable())?;
    let last = names.len().saturating_sub(1);
    for (index, name) in names.iter().enumerate() {
        match openat(
            &fd,
            name,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        ) {
            Ok(next) => fd = next,
            Err(error) if create && error == rustix::io::Errno::NOENT && index == last => {
                mkdirat(&fd, name, Mode::RWXU).map_err(|_| platform_unavailable())?;
                fd = openat(
                    &fd,
                    name,
                    OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                    Mode::empty(),
                )
                .map_err(|_| platform_unavailable())?;
            }
            #[cfg(target_os = "macos")]
            Err(error)
                if index == 0
                    && (error == rustix::io::Errno::LOOP || error == rustix::io::Errno::NOTDIR)
                    && matches!(name.as_bytes(), b"var" | b"tmp") =>
            {
                fd = openat(
                    &fd,
                    name,
                    OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
                    Mode::empty(),
                )
                .map_err(|_| platform_integrity(BackendError::Corrupt))?;
            }
            Err(_) => return Err(platform_integrity(BackendError::Corrupt)),
        }
    }
    validate_dir(&fd).map_err(platform_integrity)?;
    Ok(fd)
}

fn load_or_initialize_marker(
    root: &OwnedFd,
    config: &LocalObjectStorageConfig,
    platform_id: PlatformId,
) -> Result<FormatMarker, PlatformError> {
    match open_regular(root, FORMAT_FILE) {
        Ok(fd) => {
            drop(fd);
            let marker: FormatMarker =
                read_json_bounded(root, FORMAT_FILE, 64 * 1024).map_err(platform_integrity)?;
            if marker.schema_version != FORMAT_SCHEMA
                || marker.platform_id != platform_id.to_string()
                || marker.prefix != config.prefix
                || marker.r2_prefix != config.r2_prefix
                || !canonical_uuid_v7(&marker.root_id)
            {
                return Err(PlatformError::new(
                    ErrorCode::ObjectStorageAuthorityMismatch,
                    "object storage authority does not match the configured platform",
                ));
            }
            Ok(marker)
        }
        Err(BackendError::NotFound) => {
            for name in dir_names(root).map_err(platform_integrity)? {
                if name != OsStr::new(LOCK_FILE) {
                    return Err(platform_integrity(BackendError::Corrupt));
                }
            }
            let marker = FormatMarker {
                schema_version: FORMAT_SCHEMA,
                platform_id: platform_id.to_string(),
                root_id: uuid::Uuid::now_v7().hyphenated().to_string(),
                prefix: config.prefix.clone(),
                r2_prefix: config.r2_prefix.clone(),
            };
            write_json_create(root, FORMAT_FILE, &marker).map_err(platform_integrity)?;
            fsync(root.as_fd()).map_err(|_| platform_unavailable())?;
            Ok(marker)
        }
        Err(error) => Err(platform_integrity(error)),
    }
}

fn authority_sha256(marker: &FormatMarker) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(b"open-compute/object-authority/local/v1");
    for value in [
        marker.schema_version.to_string(),
        marker.root_id.clone(),
        marker.prefix.clone(),
        marker.r2_prefix.clone(),
    ] {
        digest.update((value.len() as u64).to_be_bytes());
        digest.update(value.as_bytes());
    }
    digest.finalize().into()
}

fn validate_root_entries(root: &OwnedFd) -> Result<(), BackendError> {
    let mut names = dir_names(root)?;
    names.sort();
    let expected = [LOCK_FILE, FORMAT_FILE, MULTIPART_DIR, OBJECTS_DIR];
    if names.len() != expected.len()
        || names
            .iter()
            .zip(expected)
            .any(|(actual, expected)| actual != OsStr::new(expected))
    {
        return Err(BackendError::Corrupt);
    }
    Ok(())
}

fn scan_objects(
    directory: &OwnedFd,
    segments: &[String],
    output: &mut Vec<ListedObject>,
    budget: &mut ScanBudget,
) -> Result<(), BackendError> {
    for name in dir_names(directory)? {
        budget.charge(0)?;
        let Some(name_str) = name.to_str() else {
            return Err(BackendError::Corrupt);
        };
        if name_str.starts_with(".partial-") {
            validate_partial_name(name_str)?;
            continue;
        }
        if name_str == OBJECT_FILE {
            if segments.is_empty() {
                return Err(BackendError::Corrupt);
            }
            let key = ObjectKey::new(segments.join("/"))?;
            let fd = open_regular(directory, OBJECT_FILE)?;
            let stat = fstat(&fd).map_err(|_| BackendError::Unavailable)?;
            budget.charge((stat.st_size as u64).min(HEADER_BYTES as u64))?;
            let metadata = read_header_from_file(&mut File::from(fd), &key)?.metadata;
            output.push(ListedObject { key, metadata });
            continue;
        }
        let mut child_segments = segments.to_owned();
        child_segments.push(name_str.to_owned());
        let candidate = ObjectKey::new(child_segments.join("/"))?;
        let _ = candidate;
        let child = open_child_dir(directory, &name)?;
        scan_objects(&child, &child_segments, output, budget)?;
    }
    Ok(())
}

fn remove_object_partials(
    directory: &OwnedFd,
    budget: &mut ScanBudget,
    cutoff_ms: i64,
) -> Result<(), BackendError> {
    for name in dir_names(directory)? {
        budget.charge(0)?;
        let Some(name_str) = name.to_str() else {
            return Err(BackendError::Corrupt);
        };
        if name_str.starts_with(".partial-") {
            validate_partial_name(name_str)?;
            remove_stale_partial(directory, name_str, cutoff_ms, budget)?;
        } else if name_str == OBJECT_FILE {
            let fd = open_regular(directory, name_str)?;
            validate_regular(&fd, None)?;
            let stat = fstat(&fd).map_err(|_| BackendError::Unavailable)?;
            budget.charge((stat.st_size as u64).min(HEADER_BYTES as u64))?;
        } else {
            let child = open_child_dir(directory, &name)?;
            remove_object_partials(&child, budget, cutoff_ms)?;
        }
    }
    fsync(directory.as_fd()).map_err(|_| BackendError::Unavailable)
}

fn remove_stale_partial(
    directory: &OwnedFd,
    name: &str,
    cutoff_ms: i64,
    budget: &mut ScanBudget,
) -> Result<(), BackendError> {
    let fd = open_regular(directory, name)?;
    let stat = fstat(&fd).map_err(|_| BackendError::Unavailable)?;
    budget.charge((stat.st_size as u64).min(HEADER_BYTES as u64))?;
    let modified_ms =
        stat.st_mtime.saturating_mul(1_000) + stat.st_mtime_nsec.saturating_div(1_000_000);
    if modified_ms <= cutoff_ms {
        unlinkat(directory, name, AtFlags::empty()).map_err(|_| BackendError::Unavailable)?;
    }
    Ok(())
}

fn validate_manifest(
    manifest: &MultipartManifest,
    key: &ObjectKey,
    upload_id: &str,
    customer_key: Option<&CustomerKey>,
) -> Result<(), BackendError> {
    validate_manifest_record(manifest, upload_id)?;
    if &manifest.key != key {
        return Err(BackendError::MultipartInvalid);
    }
    match (&manifest.encryption, customer_key) {
        (None, None) => Ok(()),
        (None, Some(_)) | (Some(_), None) => Err(BackendError::CustomerKeyInvalid),
        (Some(expected), Some(key_value)) => {
            let actual = encryption_header_with_nonce(
                key,
                key_value,
                &expected.object_version,
                &expected.nonce,
            )?;
            if actual.verifier != expected.verifier || actual.ssec_key_md5 != expected.ssec_key_md5
            {
                return Err(BackendError::CustomerKeyInvalid);
            }
            Ok(())
        }
    }
}

fn validate_manifest_record(
    manifest: &MultipartManifest,
    upload_id: &str,
) -> Result<(), BackendError> {
    if manifest.schema_version != FORMAT_SCHEMA
        || manifest.upload_id != upload_id
        || manifest.created_at_ms < 0
        || matches!(
            &manifest.status,
            MultipartStatus::Publishing { etag }
                if etag.is_empty()
                    || etag
                        .bytes()
                        .any(|byte| byte.is_ascii_control() || byte == b'"')
        )
    {
        return Err(BackendError::MultipartInvalid);
    }
    if let Some(encryption) = &manifest.encryption
        && (encryption.algorithm != "xchacha20poly1305-chunked-v1"
            || encryption.chunk_size != CHUNK_BYTES as u32
            || !canonical_uuid_v7(&encryption.object_version)
            || decode_nonce(&encryption.nonce).is_err()
            || base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(&encryption.verifier)
                .is_err()
            || !valid_lower_hex(&encryption.ssec_key_md5, 32))
    {
        return Err(BackendError::MultipartInvalid);
    }
    Ok(())
}

fn encryption_header_with_nonce(
    key: &ObjectKey,
    customer_key: &CustomerKey,
    object_version: &str,
    nonce: &str,
) -> Result<EncryptionHeader, BackendError> {
    if !canonical_uuid_v7(object_version) {
        return Err(BackendError::Corrupt);
    }
    let nonce_bytes = decode_nonce(nonce)?;
    let cipher = XChaCha20Poly1305::new(Key::from_slice(customer_key.bytes()));
    let verifier = cipher
        .encrypt(
            XNonce::from_slice(&chunk_nonce(nonce_bytes, u64::MAX)),
            Payload {
                msg: &[],
                aad: &verifier_aad(key, object_version),
            },
        )
        .map_err(|_| BackendError::CustomerKeyInvalid)?;
    Ok(EncryptionHeader {
        algorithm: "xchacha20poly1305-chunked-v1".to_owned(),
        chunk_size: CHUNK_BYTES as u32,
        object_version: object_version.to_owned(),
        nonce: nonce.to_owned(),
        verifier: base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(verifier),
        ssec_key_md5: hex::encode(Md5::digest(customer_key.bytes())),
    })
}

fn read_manifest(upload: &OwnedFd) -> Result<MultipartManifest, BackendError> {
    read_json_bounded(upload, MANIFEST_FILE, 64 * 1024)
}

fn read_json_bounded<T: DeserializeOwned>(
    parent: &OwnedFd,
    name: &str,
    limit: u64,
) -> Result<T, BackendError> {
    let fd = open_regular(parent, name)?;
    let stat = fstat(&fd).map_err(|_| BackendError::Unavailable)?;
    let size = u64::try_from(stat.st_size).map_err(|_| BackendError::Corrupt)?;
    if size == 0 || size > limit {
        return Err(BackendError::Corrupt);
    }
    let capacity = usize::try_from(size).map_err(|_| BackendError::Corrupt)?;
    let mut bytes = Vec::with_capacity(capacity);
    File::from(fd)
        .read_to_end(&mut bytes)
        .map_err(|_| BackendError::Corrupt)?;
    if bytes.len() != capacity {
        return Err(BackendError::Corrupt);
    }
    serde_json::from_slice(&bytes).map_err(|_| BackendError::Corrupt)
}

fn retire_upload_dir(root: &OwnedFd, upload_id: &str) -> Result<(), BackendError> {
    let multipart = open_child_dir(root, MULTIPART_DIR)?;
    let retired_name = format!(".gc-{upload_id}");
    renameat(&multipart, upload_id, &multipart, retired_name.as_str())
        .map_err(|_| BackendError::Unavailable)?;
    fsync(multipart.as_fd()).map_err(|_| BackendError::Unavailable)?;
    remove_retired_upload(&multipart, &retired_name)
}

fn remove_retired_upload(multipart: &OwnedFd, retired_name: &str) -> Result<(), BackendError> {
    let upload = open_child_dir(multipart, retired_name)?;
    match open_child_dir(&upload, PARTS_DIR) {
        Ok(parts) => {
            for name in dir_names(&parts)? {
                let Some(name) = name.to_str() else {
                    return Err(BackendError::Corrupt);
                };
                if !valid_part_name(name) && validate_partial_name(name).is_err() {
                    return Err(BackendError::Corrupt);
                }
                let fd = open_regular(&parts, name)?;
                validate_regular(&fd, None)?;
                unlinkat(&parts, name, AtFlags::empty()).map_err(|_| BackendError::Unavailable)?;
            }
            fsync(parts.as_fd()).map_err(|_| BackendError::Unavailable)?;
            unlinkat(&upload, PARTS_DIR, AtFlags::REMOVEDIR)
                .map_err(|_| BackendError::Unavailable)?;
        }
        Err(BackendError::NotFound) => {}
        Err(error) => return Err(error),
    }
    for name in dir_names(&upload)? {
        let Some(name) = name.to_str() else {
            return Err(BackendError::Corrupt);
        };
        if name != MANIFEST_FILE && validate_partial_name(name).is_err() {
            return Err(BackendError::Corrupt);
        }
        let fd = open_regular(&upload, name)?;
        validate_regular(&fd, None)?;
        unlinkat(&upload, name, AtFlags::empty()).map_err(|_| BackendError::Unavailable)?;
    }
    fsync(upload.as_fd()).map_err(|_| BackendError::Unavailable)?;
    unlinkat(multipart, retired_name, AtFlags::REMOVEDIR).map_err(|_| BackendError::Unavailable)?;
    fsync(multipart.as_fd()).map_err(|_| BackendError::Unavailable)
}

fn multipart_part_key(
    key: &ObjectKey,
    upload_id: &str,
    part_number: i32,
) -> Result<ObjectKey, BackendError> {
    ObjectKey::new(format!(
        "multipart/{}/{}/{part_number}",
        hex::encode(Sha256::digest(key.as_str().as_bytes())),
        upload_id.replace('-', "")
    ))
}

fn validate_upload_id(value: &str) -> Result<&str, BackendError> {
    if !canonical_uuid_v7(value) {
        return Err(BackendError::MultipartInvalid);
    }
    Ok(value)
}

fn valid_part_name(value: &str) -> bool {
    parse_part_name(value).is_ok()
}

fn parse_part_name(value: &str) -> Result<i32, BackendError> {
    let token = value.strip_suffix(".ocpart").ok_or(BackendError::Corrupt)?;
    let number = token.parse::<i32>().map_err(|_| BackendError::Corrupt)?;
    if !(1..=10_000).contains(&number) || token != number.to_string() {
        return Err(BackendError::Corrupt);
    }
    Ok(number)
}

fn validate_partial_name(value: &str) -> Result<(), BackendError> {
    let id = value
        .strip_prefix(".partial-")
        .ok_or(BackendError::Corrupt)?;
    validate_upload_id(id).map(|_| ())
}

fn multipart_etag(parts: &[UploadedPart]) -> Result<String, BackendError> {
    let mut binary_etags = Vec::with_capacity(parts.len().saturating_mul(16));
    for part in parts {
        if !valid_lower_hex(&part.etag, 32) {
            return Err(BackendError::MultipartInvalid);
        }
        binary_etags.extend(hex::decode(&part.etag).map_err(|_| BackendError::MultipartInvalid)?);
    }
    Ok(format!(
        "{}-{}",
        hex::encode(Md5::digest(binary_etags)),
        parts.len()
    ))
}

fn valid_etag(value: &str) -> bool {
    if valid_lower_hex(value, 32) {
        return true;
    }
    let Some((digest, count)) = value.split_once('-') else {
        return false;
    };
    valid_lower_hex(digest, 32)
        && count
            .parse::<usize>()
            .is_ok_and(|parsed| (1..=10_000).contains(&parsed) && parsed.to_string() == count)
}

fn valid_lower_hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn encode_cursor(key: &ObjectKey) -> String {
    format!(
        "{CURSOR_PREFIX}{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(key.as_str())
    )
}

fn decode_cursor(value: &str) -> Result<ObjectKey, BackendError> {
    let encoded = value
        .strip_prefix(CURSOR_PREFIX)
        .ok_or(BackendError::InvalidKey)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(encoded)
        .map_err(|_| BackendError::InvalidKey)?;
    let key = String::from_utf8(bytes).map_err(|_| BackendError::InvalidKey)?;
    let parsed = ObjectKey::new(key)?;
    if encode_cursor(&parsed) != value {
        return Err(BackendError::InvalidKey);
    }
    Ok(parsed)
}

fn read_chunk(reader: &mut impl Read, buffer: &mut [u8]) -> Result<usize, BackendError> {
    let mut total = 0;
    while total < buffer.len() {
        let count = reader
            .read(&mut buffer[total..])
            .map_err(|_| BackendError::Unavailable)?;
        if count == 0 {
            break;
        }
        total += count;
    }
    Ok(total)
}

fn chunk_nonce(mut base: [u8; 24], index: u64) -> [u8; 24] {
    let index = index.to_be_bytes();
    for (slot, value) in base[16..].iter_mut().zip(index) {
        *slot ^= value;
    }
    base
}

fn chunk_aad_from_digest(
    digest: &str,
    object_version: &str,
    plaintext_size: u64,
    index: u64,
) -> Vec<u8> {
    let mut aad = b"open-compute/local-object/chunk/v1".to_vec();
    aad.extend_from_slice(digest.as_bytes());
    aad.extend_from_slice(object_version.as_bytes());
    aad.extend_from_slice(&FORMAT_SCHEMA.to_be_bytes());
    aad.extend_from_slice(&plaintext_size.to_be_bytes());
    aad.extend_from_slice(&index.to_be_bytes());
    aad
}

fn verifier_aad(key: &ObjectKey, object_version: &str) -> Vec<u8> {
    verifier_aad_from_digest(
        &hex::encode(Sha256::digest(key.as_str().as_bytes())),
        object_version,
    )
}

fn verifier_aad_from_digest(digest: &str, object_version: &str) -> Vec<u8> {
    let mut aad = b"open-compute/local-object/key-verifier/v1".to_vec();
    aad.extend_from_slice(digest.as_bytes());
    aad.extend_from_slice(object_version.as_bytes());
    aad.extend_from_slice(&FORMAT_SCHEMA.to_be_bytes());
    aad
}

fn canonical_uuid_v7(value: &str) -> bool {
    uuid::Uuid::parse_str(value)
        .is_ok_and(|id| id.get_version_num() == 7 && id.hyphenated().to_string() == value)
}

fn decode_nonce(value: &str) -> Result<[u8; 24], BackendError> {
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(value)
        .map_err(|_| BackendError::Corrupt)?
        .try_into()
        .map_err(|_| BackendError::Corrupt)
}

fn key_lock_index(key: &ObjectKey) -> usize {
    usize::from(Sha256::digest(key.as_str().as_bytes())[0]) % 64
}

fn write_json_create(
    parent: &OwnedFd,
    name: &str,
    value: &impl Serialize,
) -> Result<(), BackendError> {
    let bytes = serde_json::to_vec(value).map_err(|_| BackendError::Corrupt)?;
    let fd = create_regular(parent, name)?;
    let mut file = File::from(fd);
    file.write_all(&bytes)
        .and_then(|()| file.sync_all())
        .map_err(|_| BackendError::Unavailable)
}

fn write_json_replace(
    parent: &OwnedFd,
    name: &str,
    value: &impl Serialize,
) -> Result<(), BackendError> {
    let bytes = serde_json::to_vec(value).map_err(|_| BackendError::Corrupt)?;
    if bytes.len() > HEADER_BYTES {
        return Err(BackendError::Corrupt);
    }
    let partial = format!(".partial-{}", uuid::Uuid::now_v7());
    let mut guard = PartialGuard::new(dup_fd(parent)?, partial.clone());
    let fd = create_regular(parent, &partial)?;
    let mut file = File::from(fd);
    file.write_all(&bytes)
        .and_then(|()| file.sync_all())
        .map_err(|_| BackendError::Unavailable)?;
    renameat(parent, partial.as_str(), parent, name).map_err(|_| BackendError::Unavailable)?;
    guard.persist = true;
    fsync(parent.as_fd()).map_err(|_| BackendError::Unavailable)
}

fn create_regular(parent: &OwnedFd, name: &str) -> Result<OwnedFd, BackendError> {
    openat(
        parent,
        name,
        OFlags::RDWR | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::RUSR | Mode::WUSR,
    )
    .map_err(|_| BackendError::Unavailable)
}

fn open_regular(parent: &OwnedFd, name: &str) -> Result<OwnedFd, BackendError> {
    let fd = openat(
        parent,
        name,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK,
        Mode::empty(),
    )
    .map_err(|error| {
        if error == rustix::io::Errno::NOENT {
            BackendError::NotFound
        } else if error == rustix::io::Errno::LOOP
            || error == rustix::io::Errno::NOTDIR
            || statat(parent, name, AtFlags::SYMLINK_NOFOLLOW).is_ok_and(|stat| {
                rustix::fs::FileType::from_raw_mode(stat.st_mode)
                    != rustix::fs::FileType::RegularFile
            })
        {
            BackendError::Corrupt
        } else {
            BackendError::Unavailable
        }
    })?;
    validate_regular(&fd, None)?;
    Ok(fd)
}

fn validate_regular(fd: &OwnedFd, expected_size: Option<u64>) -> Result<(), BackendError> {
    let stat = fstat(fd).map_err(|_| BackendError::Unavailable)?;
    if rustix::fs::FileType::from_raw_mode(stat.st_mode) != rustix::fs::FileType::RegularFile
        || stat.st_nlink != 1
        || stat.st_mode as u32 & 0o777 != 0o600
        || stat.st_uid != rustix::process::getuid().as_raw()
        || expected_size.is_some_and(|expected| stat.st_size as u64 != expected)
    {
        return Err(BackendError::Corrupt);
    }
    Ok(())
}

fn validate_dir(fd: &OwnedFd) -> Result<(), BackendError> {
    let stat = fstat(fd).map_err(|_| BackendError::Unavailable)?;
    if rustix::fs::FileType::from_raw_mode(stat.st_mode) != rustix::fs::FileType::Directory
        || stat.st_mode as u32 & 0o777 != 0o700
        || stat.st_uid != rustix::process::getuid().as_raw()
    {
        return Err(BackendError::Corrupt);
    }
    Ok(())
}

fn ensure_dir(parent: &OwnedFd, name: &str) -> Result<(), BackendError> {
    let created = match mkdirat(parent, name, Mode::RWXU) {
        Ok(()) => true,
        Err(error) if error == rustix::io::Errno::EXIST => false,
        Err(_) => return Err(BackendError::Unavailable),
    };
    let child = open_child_dir(parent, name)?;
    if created {
        fchmod(&child, Mode::RWXU).map_err(|_| BackendError::Unavailable)?;
    }
    validate_dir(&child)
}

fn open_child_dir<P: rustix::path::Arg>(
    parent: &OwnedFd,
    name: P,
) -> Result<OwnedFd, BackendError> {
    let fd = openat(
        parent,
        name,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|error| {
        if error == rustix::io::Errno::NOENT {
            BackendError::NotFound
        } else if error == rustix::io::Errno::LOOP || error == rustix::io::Errno::NOTDIR {
            BackendError::Corrupt
        } else {
            BackendError::Unavailable
        }
    })?;
    validate_dir(&fd)?;
    Ok(fd)
}

fn dir_names(directory: &OwnedFd) -> Result<Vec<OsString>, BackendError> {
    let entries = rustix::fs::Dir::read_from(directory).map_err(|_| BackendError::Unavailable)?;
    let mut names = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|_| BackendError::Unavailable)?;
        let bytes = entry.file_name().to_bytes();
        if bytes != b"." && bytes != b".." {
            if names.len() >= MAX_SCAN_ENTRIES {
                return Err(BackendError::Capacity);
            }
            names.push(OsString::from_vec(bytes.to_vec()));
        }
    }
    names.sort();
    Ok(names)
}

fn dup_fd(fd: &OwnedFd) -> Result<OwnedFd, BackendError> {
    rustix::io::dup(fd).map_err(|_| BackendError::Unavailable)
}

struct PartialGuard {
    parent: OwnedFd,
    name: String,
    persist: bool,
}

impl PartialGuard {
    fn new(parent: OwnedFd, name: String) -> Self {
        Self {
            parent,
            name,
            persist: false,
        }
    }
}

impl Drop for PartialGuard {
    fn drop(&mut self) {
        if !self.persist {
            let _ = unlinkat(&self.parent, self.name.as_str(), AtFlags::empty());
        }
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
        .unwrap_or(i64::MAX)
}

fn platform_integrity(_error: BackendError) -> PlatformError {
    PlatformError::new(
        ErrorCode::ObjectStorageIntegrityError,
        "local object authority failed integrity validation",
    )
}

fn platform_unavailable() -> PlatformError {
    PlatformError::new(
        ErrorCode::ObjectStorageUnavailable,
        "object storage authority is unavailable",
    )
}

fn require_local_filesystem(fd: &impl rustix::fd::AsFd) -> Result<(), PlatformError> {
    let stat = rustix::fs::fstatfs(fd).map_err(|_| platform_unavailable())?;
    if filesystem_is_network_or_unknown(&stat) {
        return Err(PlatformError::new(
            ErrorCode::ConfigInvalid,
            "local object authority requires a classified local filesystem",
        ));
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn filesystem_is_network_or_unknown(stat: &rustix::fs::StatFs) -> bool {
    const NFS: i64 = 0x6969;
    const CIFS: i64 = 0xFF5_34D42;
    const SMB: i64 = 0x517B;
    const FUSE: i64 = 0x6573_5546;
    const AFS: i64 = 0x5346_414F;
    matches!(stat.f_type, NFS | CIFS | SMB | FUSE | AFS)
}

#[cfg(not(target_os = "linux"))]
fn filesystem_is_network_or_unknown(stat: &rustix::fs::StatFs) -> bool {
    let raw = stat.f_fstypename;
    let bytes: Vec<u8> = raw
        .iter()
        .copied()
        .take_while(|byte| *byte != 0)
        .map(|byte| byte as u8)
        .collect();
    let name = String::from_utf8_lossy(&bytes).to_ascii_lowercase();
    name.is_empty()
        || name.contains("nfs")
        || name.contains("smb")
        || name.contains("afp")
        || name.contains("fuse")
        || name.contains("webdav")
        || name.contains("cifs")
}

trait MetadataExt {
    fn nlink(&self) -> u64;
}

impl MetadataExt for std::fs::Metadata {
    fn nlink(&self) -> u64 {
        std::os::unix::fs::MetadataExt::nlink(self)
    }
}
