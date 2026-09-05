//! Canonical object-storage layout for authenticated full-platform snapshots.

use crate::backend::{
    BackendError, GetOptions, HeadOptions, ObjectBackend, ObjectHttpMetadata, ObjectKey,
    ObjectMetadata, ObjectSource, PutMode, PutOptions, open_private_source,
};
use crate::error;
use open_compute_core::{ErrorCode, ObjectStorageKind, PlatformError, PlatformId};
use sha2::{Digest as _, Sha256};
use std::collections::BTreeMap;
use std::io::{Read as _, Seek as _, SeekFrom};
use std::os::unix::fs::OpenOptionsExt as _;
use std::path::Path;
use std::str::FromStr as _;
use std::time::{Duration, SystemTime};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

const META_SHA256: &str = "sha256";
const SNAPSHOT_LAYOUT: &str = "snapshots/v1";
const MAX_LISTED_SNAPSHOTS: usize = 10_000;
const MAX_LISTED_SNAPSHOT_OBJECTS: usize = 100_000;

/// Committed snapshot manifest discovered by bounded listing.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommittedSnapshot {
    /// Canonical snapshot `UUIDv7`.
    pub snapshot_id: String,
    /// Full manifest object key.
    pub manifest_key: String,
}

/// Aggregate result from exact-layout cleanup of old, uncommitted snapshot uploads.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct IncompleteSnapshotCleanup {
    /// Number of valid incomplete snapshot prefixes removed.
    pub prefixes: u64,
    /// Number of exact snapshot objects removed.
    pub objects: u64,
    /// Aggregate stored bytes removed.
    pub bytes: u64,
}

#[derive(Default)]
struct IncompletePrefix {
    committed: bool,
    invalid: bool,
    objects: Vec<(ObjectKey, u64, Option<SystemTime>)>,
}

/// Typed object access restricted to one platform's versioned snapshot layout.
#[derive(Clone, Debug)]
pub struct SnapshotObjectStore {
    backend: ObjectBackend,
    platform_id: PlatformId,
}

impl SnapshotObjectStore {
    /// Bind the selected object backend to one stable platform identity.
    #[must_use]
    pub const fn new(backend: ObjectBackend, platform_id: PlatformId) -> Self {
        Self {
            backend,
            platform_id,
        }
    }

    /// Platform identity selected for this snapshot namespace.
    #[must_use]
    pub const fn platform_id(&self) -> PlatformId {
        self.platform_id
    }

    /// Canonical system object prefix shared by snapshot external references.
    #[must_use]
    pub fn system_prefix(&self) -> &str {
        self.backend.prefix()
    }

    /// Discover the unique platform manifest for a snapshot UUID without local state.
    pub async fn discover(
        backend: ObjectBackend,
        snapshot_id: &str,
    ) -> Result<Self, PlatformError> {
        validate_snapshot_id(snapshot_id)?;
        let prefix = format!("{}{SNAPSHOT_LAYOUT}/", backend.prefix());
        let suffix = format!("/{snapshot_id}/manifest.json");
        let mut cursor = None;
        let mut found = None;
        loop {
            let page = backend
                .list(&prefix, 1000, cursor.as_deref())
                .await
                .map_err(error::from_backend)?;
            for object in page.objects {
                let Some(platform) = object
                    .key
                    .as_str()
                    .strip_prefix(&prefix)
                    .and_then(|value| value.strip_suffix(&suffix))
                else {
                    continue;
                };
                if platform.contains('/') {
                    continue;
                }
                let platform_id = PlatformId::from_str(platform).map_err(|_| snapshot_invalid())?;
                if found.replace(platform_id).is_some() {
                    return Err(snapshot_invalid());
                }
            }
            cursor = page.next_cursor;
            if cursor.is_none() {
                break;
            }
        }
        found
            .map(|platform_id| Self::new(backend, platform_id))
            .ok_or_else(|| {
                PlatformError::new(
                    ErrorCode::SnapshotInvalid,
                    "committed platform snapshot was not found",
                )
            })
    }

    /// Selected backend kind captured by snapshot manifests.
    #[must_use]
    pub fn backend_kind(&self) -> ObjectStorageKind {
        self.backend.kind()
    }

    /// Lowercase SHA-256 of the configured object authority.
    #[must_use]
    pub fn authority_fingerprint(&self) -> String {
        hex::encode(self.backend.authority_sha256())
    }

    /// Lowercase SHA-256 of the isolated R2 prefix.
    #[must_use]
    pub fn r2_prefix_fingerprint(&self) -> String {
        let mut digest = Sha256::new();
        digest.update(b"open-compute/r2-prefix/v1\0");
        digest.update(self.backend.r2_prefix().len().to_be_bytes());
        digest.update(self.backend.r2_prefix().as_bytes());
        hex::encode(digest.finalize())
    }

    /// Full object prefix for a new canonical snapshot identity.
    pub fn object_prefix(&self, snapshot_id: &str) -> Result<String, PlatformError> {
        validate_snapshot_id(snapshot_id)?;
        Ok(format!(
            "{}{SNAPSHOT_LAYOUT}/{}/{snapshot_id}/objects/",
            self.backend.prefix(),
            self.platform_id
        ))
    }

    /// Full manifest key for a canonical snapshot identity.
    pub fn manifest_key(&self, snapshot_id: &str) -> Result<String, PlatformError> {
        validate_snapshot_id(snapshot_id)?;
        Ok(format!(
            "{}{SNAPSHOT_LAYOUT}/{}/{snapshot_id}/manifest.json",
            self.backend.prefix(),
            self.platform_id
        ))
    }

    /// Upload one pre-hashed snapshot object and verify its metadata and bytes.
    pub async fn put_file(
        &self,
        key: &str,
        path: &Path,
        expected_sha256: &str,
        expected_size: u64,
    ) -> Result<(), PlatformError> {
        self.validate_file_key(key)?;
        validate_sha256(expected_sha256)?;
        if expected_size > self.backend.max_object_bytes() {
            return Err(snapshot_invalid());
        }
        let mut file = open_private_source(path, expected_size).map_err(|_| snapshot_invalid())?;
        verify_reader(&mut file, expected_sha256, expected_size)?;
        file.seek(SeekFrom::Start(0))
            .map_err(|_| snapshot_invalid())?;
        let result = self
            .backend
            .put(
                &object_key(key)?,
                ObjectSource::File {
                    file,
                    length: expected_size,
                },
                immutable_options(expected_size, expected_sha256, None),
            )
            .await;
        if let Err(failure) = result
            && failure != BackendError::PreconditionFailed
        {
            return Err(error::from_backend(failure));
        }
        self.verify_file(key, expected_sha256, expected_size).await
    }

    /// Put the canonical manifest last, using create-only semantics.
    pub async fn put_manifest(
        &self,
        snapshot_id: &str,
        bytes: &[u8],
        max_bytes: u64,
    ) -> Result<String, PlatformError> {
        if bytes.is_empty() || bytes.len() as u64 > max_bytes {
            return Err(snapshot_invalid());
        }
        let key = self.manifest_key(snapshot_id)?;
        let digest = hex::encode(Sha256::digest(bytes));
        let result = self
            .backend
            .put(
                &object_key(&key)?,
                ObjectSource::Bytes(bytes::Bytes::copy_from_slice(bytes)),
                immutable_options(bytes.len() as u64, &digest, Some("application/json")),
            )
            .await;
        if let Err(failure) = result
            && failure != BackendError::PreconditionFailed
        {
            return Err(error::from_backend(failure));
        }
        if self.get_manifest(snapshot_id, max_bytes).await? != bytes {
            return Err(snapshot_invalid());
        }
        Ok(key)
    }

    /// Download a bounded committed manifest.
    pub async fn get_manifest(
        &self,
        snapshot_id: &str,
        max_bytes: u64,
    ) -> Result<Vec<u8>, PlatformError> {
        let output = self
            .backend
            .get(
                &object_key(&self.manifest_key(snapshot_id)?)?,
                GetOptions::default(),
            )
            .await
            .map_err(error::from_backend)?;
        if output.metadata.size == 0 || output.metadata.size > max_bytes {
            return Err(snapshot_invalid());
        }
        let bytes = output
            .body
            .collect()
            .await
            .map_err(|_| snapshot_invalid())?
            .into_bytes()
            .to_vec();
        if bytes.len() as u64 != output.metadata.size {
            return Err(snapshot_invalid());
        }
        Ok(bytes)
    }

    /// Stream one verified snapshot file into a newly created local file.
    pub async fn download_file(
        &self,
        key: &str,
        destination: &Path,
        expected_sha256: &str,
        expected_size: u64,
    ) -> Result<(), PlatformError> {
        self.validate_file_key(key)?;
        validate_sha256(expected_sha256)?;
        if expected_size > self.backend.max_object_bytes() {
            return Err(snapshot_invalid());
        }
        let output = self
            .backend
            .get(&object_key(key)?, GetOptions::default())
            .await
            .map_err(error::from_backend)?;
        validate_exact_metadata(&output.metadata, expected_sha256, expected_size, true)?;
        let file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(destination)
            .map_err(|_| snapshot_invalid())?;
        let mut file = tokio::fs::File::from_std(file);
        let mut reader = output.body.into_async_read();
        let digest = copy_and_hash(&mut reader, Some(&mut file), expected_size).await?;
        file.sync_all().await.map_err(|_| snapshot_invalid())?;
        if digest != expected_sha256 {
            return Err(snapshot_invalid());
        }
        Ok(())
    }

    /// HEAD and stream-verify one snapshot object without retaining bytes.
    pub async fn verify_file(
        &self,
        key: &str,
        expected_sha256: &str,
        expected_size: u64,
    ) -> Result<(), PlatformError> {
        self.validate_file_key(key)?;
        let head = self
            .backend
            .head(&object_key(key)?, HeadOptions::default())
            .await
            .map_err(error::from_backend)?;
        validate_exact_metadata(&head, expected_sha256, expected_size, true)?;
        self.verify_body(key, expected_sha256, expected_size).await
    }

    /// Stream-verify one manifest-pinned immutable object under the owned system or R2 prefix.
    pub async fn verify_external_reference(
        &self,
        key: &str,
        expected_sha256: &str,
        expected_size: u64,
    ) -> Result<(), PlatformError> {
        if key.contains("..")
            || key.len() > 1024
            || !(key.starts_with(self.backend.prefix())
                || key.starts_with(self.backend.r2_prefix()))
        {
            return Err(snapshot_invalid());
        }
        validate_sha256(expected_sha256)?;
        let head = self
            .backend
            .head(&object_key(key)?, HeadOptions::default())
            .await
            .map_err(error::from_backend)?;
        validate_exact_metadata(&head, expected_sha256, expected_size, false)?;
        self.verify_body(key, expected_sha256, expected_size).await
    }

    async fn verify_body(
        &self,
        key: &str,
        expected_sha256: &str,
        expected_size: u64,
    ) -> Result<(), PlatformError> {
        let output = self
            .backend
            .get(&object_key(key)?, GetOptions::default())
            .await
            .map_err(error::from_backend)?;
        let mut reader = output.body.into_async_read();
        if copy_and_hash(&mut reader, None, expected_size).await? != expected_sha256 {
            return Err(snapshot_invalid());
        }
        Ok(())
    }

    /// List only committed manifest keys for this platform, bounded by a fixed hard cap.
    pub async fn list_committed(&self) -> Result<Vec<CommittedSnapshot>, PlatformError> {
        let prefix = self.platform_prefix();
        let mut cursor = None;
        let mut snapshots = Vec::new();
        loop {
            let page = self
                .backend
                .list(&prefix, 1000, cursor.as_deref())
                .await
                .map_err(error::from_backend)?;
            for object in page.objects {
                let Some(snapshot_id) = parse_manifest_key(&prefix, object.key.as_str()) else {
                    continue;
                };
                snapshots.push(CommittedSnapshot {
                    snapshot_id,
                    manifest_key: object.key.as_str().to_owned(),
                });
                if snapshots.len() > MAX_LISTED_SNAPSHOTS {
                    return Err(snapshot_invalid());
                }
            }
            cursor = page.next_cursor;
            if cursor.is_none() {
                break;
            }
        }
        snapshots.sort_by(|left, right| left.snapshot_id.cmp(&right.snapshot_id));
        Ok(snapshots)
    }

    /// Delete old, exact-layout object prefixes that have no manifest commit marker.
    pub async fn cleanup_incomplete(
        &self,
        grace_deadline: SystemTime,
    ) -> Result<IncompleteSnapshotCleanup, PlatformError> {
        let prefix = self.platform_prefix();
        let objects = self.list_all(&prefix).await?;
        let mut prefixes: BTreeMap<String, IncompletePrefix> = BTreeMap::new();
        for object in objects {
            let Some(relative) = object.key.as_str().strip_prefix(&prefix) else {
                continue;
            };
            let Some((snapshot_id, _)) = relative.split_once('/') else {
                continue;
            };
            if validate_snapshot_id(snapshot_id).is_err() {
                continue;
            }
            let entry = prefixes.entry(snapshot_id.to_owned()).or_default();
            if relative == format!("{snapshot_id}/manifest.json") {
                entry.committed = true;
            } else if parse_file_key(&prefix, object.key.as_str()).as_deref() == Some(snapshot_id) {
                let modified = u64::try_from(object.metadata.last_modified_ms)
                    .ok()
                    .and_then(|milliseconds| {
                        SystemTime::UNIX_EPOCH.checked_add(Duration::from_millis(milliseconds))
                    });
                entry
                    .objects
                    .push((object.key, object.metadata.size, modified));
            } else {
                entry.invalid = true;
            }
        }
        let mut cleanup = IncompleteSnapshotCleanup::default();
        for entry in prefixes.into_values() {
            if entry.committed
                || entry.invalid
                || entry.objects.is_empty()
                || entry
                    .objects
                    .iter()
                    .any(|(_, _, modified)| modified.is_none_or(|value| value > grace_deadline))
            {
                continue;
            }
            for (key, size, _) in entry.objects {
                self.backend
                    .delete(&key)
                    .await
                    .map_err(error::from_backend)?;
                cleanup.objects = cleanup.objects.saturating_add(1);
                cleanup.bytes = cleanup.bytes.saturating_add(size);
            }
            cleanup.prefixes = cleanup.prefixes.saturating_add(1);
        }
        Ok(cleanup)
    }

    /// Delete one exact canonical snapshot-owned object or manifest.
    pub async fn delete_exact(&self, key: &str) -> Result<(), PlatformError> {
        if self.validate_file_key(key).is_err() && self.validate_manifest_key(key).is_err() {
            return Err(snapshot_invalid());
        }
        self.backend
            .delete(&object_key(key)?)
            .await
            .map_err(error::from_backend)
    }

    async fn list_all(&self, prefix: &str) -> Result<Vec<crate::ListedObject>, PlatformError> {
        let mut cursor = None;
        let mut objects = Vec::new();
        loop {
            let page = self
                .backend
                .list(prefix, 1000, cursor.as_deref())
                .await
                .map_err(error::from_backend)?;
            objects.extend(page.objects);
            if objects.len() > MAX_LISTED_SNAPSHOT_OBJECTS {
                return Err(snapshot_invalid());
            }
            cursor = page.next_cursor;
            if cursor.is_none() {
                return Ok(objects);
            }
        }
    }

    fn platform_prefix(&self) -> String {
        format!(
            "{}{SNAPSHOT_LAYOUT}/{}/",
            self.backend.prefix(),
            self.platform_id
        )
    }

    fn validate_file_key(&self, key: &str) -> Result<(), PlatformError> {
        parse_file_key(&self.platform_prefix(), key)
            .map(|_| ())
            .ok_or_else(snapshot_invalid)
    }

    fn validate_manifest_key(&self, key: &str) -> Result<(), PlatformError> {
        parse_manifest_key(&self.platform_prefix(), key)
            .map(|_| ())
            .ok_or_else(snapshot_invalid)
    }
}

fn immutable_options(size: u64, digest: &str, content_type: Option<&str>) -> PutOptions {
    PutOptions {
        mode: PutMode::CreateOnly,
        metadata: ObjectMetadata {
            size,
            user: BTreeMap::from([(META_SHA256.to_owned(), digest.to_owned())]),
            http: ObjectHttpMetadata {
                content_type: content_type.map(str::to_owned),
                ..ObjectHttpMetadata::default()
            },
            ..ObjectMetadata::default()
        },
        customer_key: None,
    }
}

fn validate_exact_metadata(
    metadata: &ObjectMetadata,
    expected_sha256: &str,
    expected_size: u64,
    require_digest: bool,
) -> Result<(), PlatformError> {
    let digest = metadata.user.get(META_SHA256);
    if metadata.size != expected_size
        || require_digest && digest.is_none_or(|value| value != expected_sha256)
    {
        return Err(snapshot_invalid());
    }
    Ok(())
}

async fn copy_and_hash<R: tokio::io::AsyncRead + Unpin>(
    reader: &mut R,
    mut output: Option<&mut tokio::fs::File>,
    expected_size: u64,
) -> Result<String, PlatformError> {
    let mut hasher = Sha256::new();
    let mut total = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = reader
            .read(&mut buffer)
            .await
            .map_err(|_| snapshot_invalid())?;
        if read == 0 {
            break;
        }
        total = total
            .checked_add(read as u64)
            .ok_or_else(snapshot_invalid)?;
        if total > expected_size {
            return Err(snapshot_invalid());
        }
        hasher.update(&buffer[..read]);
        if let Some(file) = output.as_deref_mut() {
            file.write_all(&buffer[..read])
                .await
                .map_err(|_| snapshot_invalid())?;
        }
    }
    if total != expected_size {
        return Err(snapshot_invalid());
    }
    Ok(hex::encode(hasher.finalize()))
}

fn object_key(key: &str) -> Result<ObjectKey, PlatformError> {
    ObjectKey::new(key.to_owned()).map_err(error::from_backend)
}

fn parse_manifest_key(prefix: &str, key: &str) -> Option<String> {
    let snapshot_id = key.strip_prefix(prefix)?.strip_suffix("/manifest.json")?;
    validate_snapshot_id(snapshot_id)
        .is_ok()
        .then(|| snapshot_id.to_owned())
}

fn parse_file_key(prefix: &str, key: &str) -> Option<String> {
    let (snapshot_id, object) = key.strip_prefix(prefix)?.split_once("/objects/")?;
    validate_snapshot_id(snapshot_id).ok()?;
    if object.len() != 10
        || !object.ends_with(".bin")
        || !object[..6].bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    Some(snapshot_id.to_owned())
}

fn validate_snapshot_id(value: &str) -> Result<(), PlatformError> {
    let id = uuid::Uuid::parse_str(value).map_err(|_| snapshot_invalid())?;
    if id.get_version_num() != 7 || id.hyphenated().to_string() != value {
        return Err(snapshot_invalid());
    }
    Ok(())
}

fn validate_sha256(value: &str) -> Result<(), PlatformError> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(snapshot_invalid());
    }
    Ok(())
}

fn verify_reader(
    reader: &mut std::fs::File,
    expected_sha256: &str,
    expected_size: u64,
) -> Result<(), PlatformError> {
    let mut hasher = Sha256::new();
    let mut total = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = reader.read(&mut buffer).map_err(|_| snapshot_invalid())?;
        if read == 0 {
            break;
        }
        total = total
            .checked_add(read as u64)
            .ok_or_else(snapshot_invalid)?;
        if total > expected_size {
            return Err(snapshot_invalid());
        }
        hasher.update(&buffer[..read]);
    }
    if total != expected_size || hex::encode(hasher.finalize()) != expected_sha256 {
        return Err(snapshot_invalid());
    }
    Ok(())
}

fn snapshot_invalid() -> PlatformError {
    PlatformError::new(
        ErrorCode::SnapshotInvalid,
        "platform snapshot object is invalid",
    )
}
