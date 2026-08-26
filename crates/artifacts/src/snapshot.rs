//! Canonical S3 layout for authenticated full-platform snapshots.

use crate::client::S3ArtifactClient;
use crate::error;
use aws_sdk_s3::error::SdkError;
use aws_sdk_s3::primitives::ByteStream;
use aws_smithy_types::byte_stream::Length;
use open_compute_core::{ErrorCode, PlatformError, PlatformId};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::io::{Read, Seek as _, SeekFrom};
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
    /// Aggregate provider-reported bytes removed.
    pub bytes: u64,
}

#[derive(Default)]
struct IncompletePrefix {
    committed: bool,
    invalid: bool,
    objects: Vec<(String, u64, Option<SystemTime>)>,
}

/// Typed S3 access restricted to one platform's versioned snapshot layout.
#[derive(Clone, Debug)]
pub struct SnapshotObjectStore {
    client: S3ArtifactClient,
    platform_id: PlatformId,
}

impl SnapshotObjectStore {
    /// Bind a configured S3 client to one stable platform identity.
    #[must_use]
    pub fn new(client: S3ArtifactClient, platform_id: PlatformId) -> Self {
        Self {
            client,
            platform_id,
        }
    }

    /// Discover the unique platform manifest for a snapshot UUID without local state.
    pub async fn discover(
        client: S3ArtifactClient,
        snapshot_id: &str,
    ) -> Result<Self, PlatformError> {
        validate_snapshot_id(snapshot_id)?;
        let prefix = format!("{}{SNAPSHOT_LAYOUT}/", client.prefix());
        let suffix = format!("/{snapshot_id}/manifest.json");
        let mut token = None;
        let mut found = None;
        loop {
            let mut request = client
                .inner()
                .list_objects_v2()
                .bucket(client.bucket())
                .prefix(&prefix)
                .max_keys(1000);
            if let Some(value) = &token {
                request = request.continuation_token(value);
            }
            let output = request
                .send()
                .await
                .map_err(|failure| error::from_list(&failure))?;
            for object in output.contents() {
                let Some(key) = object.key() else { continue };
                let Some(platform) = key
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
            if output.is_truncated() == Some(true) {
                token = output.next_continuation_token().map(ToOwned::to_owned);
                if token.is_none() {
                    return Err(snapshot_invalid());
                }
            } else {
                break;
            }
        }
        found
            .map(|platform_id| Self::new(client, platform_id))
            .ok_or_else(|| {
                PlatformError::new(
                    ErrorCode::SnapshotInvalid,
                    "committed platform snapshot was not found",
                )
            })
    }

    /// Lowercase SHA-256 of the configured remote authority.
    #[must_use]
    pub fn authority_fingerprint(&self) -> String {
        hex::encode(self.client.authority_sha256())
    }

    /// Lowercase SHA-256 of the isolated R2 prefix.
    #[must_use]
    pub fn r2_prefix_fingerprint(&self) -> String {
        let mut digest = Sha256::new();
        digest.update(b"open-compute/r2-prefix/v1\0");
        digest.update(self.client.r2_prefix().len().to_be_bytes());
        digest.update(self.client.r2_prefix().as_bytes());
        hex::encode(digest.finalize())
    }

    /// Full object prefix for a new canonical snapshot identity.
    pub fn object_prefix(&self, snapshot_id: &str) -> Result<String, PlatformError> {
        validate_snapshot_id(snapshot_id)?;
        Ok(format!(
            "{}{SNAPSHOT_LAYOUT}/{}/{snapshot_id}/objects/",
            self.client.prefix(),
            self.platform_id
        ))
    }

    /// Full manifest key for a canonical snapshot identity.
    pub fn manifest_key(&self, snapshot_id: &str) -> Result<String, PlatformError> {
        validate_snapshot_id(snapshot_id)?;
        Ok(format!(
            "{}{SNAPSHOT_LAYOUT}/{}/{snapshot_id}/manifest.json",
            self.client.prefix(),
            self.platform_id
        ))
    }

    /// Upload one pre-hashed snapshot object and verify its remote metadata and bytes.
    pub async fn put_file(
        &self,
        key: &str,
        path: &Path,
        expected_sha256: &str,
        expected_size: u64,
    ) -> Result<(), PlatformError> {
        self.validate_file_key(key)?;
        validate_sha256(expected_sha256)?;
        if expected_size > self.client.max_artifact_bytes() {
            return Err(PlatformError::new(
                ErrorCode::LimitInvalid,
                "snapshot object exceeds configured maximum size",
            ));
        }
        let mut file = std::fs::File::open(path).map_err(|_| snapshot_invalid())?;
        let metadata = file.metadata().map_err(|_| snapshot_invalid())?;
        if !metadata.file_type().is_file() || metadata.len() != expected_size {
            return Err(snapshot_invalid());
        }
        verify_reader(&mut file, expected_sha256, expected_size)?;
        file.seek(SeekFrom::Start(0))
            .map_err(|_| snapshot_invalid())?;
        let body = ByteStream::read_from()
            .file(tokio::fs::File::from_std(file))
            .length(Length::Exact(expected_size))
            .buffer_size(64 * 1024)
            .build()
            .await
            .map_err(|_| snapshot_invalid())?;
        let result = self
            .client
            .inner()
            .put_object()
            .bucket(self.client.bucket())
            .key(key)
            .body(body)
            .content_length(i64::try_from(expected_size).map_err(|_| snapshot_invalid())?)
            .metadata(META_SHA256, expected_sha256)
            .if_none_match("*")
            .send()
            .await;
        if let Err(failure) = result
            && !matches!(
                &failure,
                SdkError::ServiceError(service)
                    if matches!(service.raw().status().as_u16(), 409 | 412)
            )
        {
            return Err(error::from_put(&failure));
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
            .client
            .inner()
            .put_object()
            .bucket(self.client.bucket())
            .key(&key)
            .body(ByteStream::from(bytes.to_vec()))
            .content_length(i64::try_from(bytes.len()).map_err(|_| snapshot_invalid())?)
            .content_type("application/json")
            .metadata(META_SHA256, &digest)
            .if_none_match("*")
            .send()
            .await;
        if let Err(failure) = result
            && !matches!(
                &failure,
                SdkError::ServiceError(service)
                    if matches!(service.raw().status().as_u16(), 409 | 412)
            )
        {
            return Err(error::from_put(&failure));
        }
        let read = self.get_manifest(snapshot_id, max_bytes).await?;
        if read != bytes {
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
        let key = self.manifest_key(snapshot_id)?;
        let output = self
            .client
            .inner()
            .get_object()
            .bucket(self.client.bucket())
            .key(key)
            .send()
            .await
            .map_err(|failure| error::from_get(&failure))?;
        let length = u64::try_from(output.content_length().unwrap_or(-1)).unwrap_or(u64::MAX);
        if length == 0 || length > max_bytes {
            return Err(snapshot_invalid());
        }
        let collected = output
            .body
            .collect()
            .await
            .map_err(|_| snapshot_invalid())?;
        let bytes = collected.into_bytes().to_vec();
        if bytes.len() as u64 != length {
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
        if expected_size > self.client.max_artifact_bytes() {
            return Err(snapshot_invalid());
        }
        let output = self
            .client
            .inner()
            .get_object()
            .bucket(self.client.bucket())
            .key(key)
            .send()
            .await
            .map_err(|failure| error::from_get(&failure))?;
        let length = u64::try_from(output.content_length().unwrap_or(-1)).unwrap_or(u64::MAX);
        let digest = output
            .metadata()
            .and_then(|metadata| metadata.get(META_SHA256));
        if length != expected_size || digest.is_none_or(|value| value != expected_sha256) {
            return Err(snapshot_invalid());
        }
        let file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(destination)
            .map_err(|_| snapshot_invalid())?;
        let mut file = tokio::fs::File::from_std(file);
        let mut reader = output.body.into_async_read();
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
            file.write_all(&buffer[..read])
                .await
                .map_err(|_| snapshot_invalid())?;
        }
        file.sync_all().await.map_err(|_| snapshot_invalid())?;
        if total != expected_size || hex::encode(hasher.finalize()) != expected_sha256 {
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
            .client
            .inner()
            .head_object()
            .bucket(self.client.bucket())
            .key(key)
            .send()
            .await
            .map_err(|failure| error::from_head(&failure))?;
        let size = u64::try_from(head.content_length().unwrap_or(-1)).unwrap_or(u64::MAX);
        let digest = head
            .metadata()
            .and_then(|metadata| metadata.get(META_SHA256));
        if size != expected_size || digest.is_none_or(|value| value != expected_sha256) {
            return Err(snapshot_invalid());
        }
        let output = self
            .client
            .inner()
            .get_object()
            .bucket(self.client.bucket())
            .key(key)
            .send()
            .await
            .map_err(|failure| error::from_get(&failure))?;
        let mut reader = output.body.into_async_read();
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
        }
        if total != expected_size || hex::encode(hasher.finalize()) != expected_sha256 {
            return Err(snapshot_invalid());
        }
        Ok(())
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
            || !(key.starts_with(self.client.prefix()) || key.starts_with(self.client.r2_prefix()))
        {
            return Err(snapshot_invalid());
        }
        validate_sha256(expected_sha256)?;
        let head = self
            .client
            .inner()
            .head_object()
            .bucket(self.client.bucket())
            .key(key)
            .send()
            .await
            .map_err(|failure| error::from_head(&failure))?;
        let size = u64::try_from(head.content_length().unwrap_or(-1)).unwrap_or(u64::MAX);
        if size != expected_size {
            return Err(snapshot_invalid());
        }
        let output = self
            .client
            .inner()
            .get_object()
            .bucket(self.client.bucket())
            .key(key)
            .send()
            .await
            .map_err(|failure| error::from_get(&failure))?;
        let mut reader = output.body.into_async_read();
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
        }
        if total != expected_size || hex::encode(hasher.finalize()) != expected_sha256 {
            return Err(snapshot_invalid());
        }
        Ok(())
    }

    /// List only committed manifest keys for this platform, bounded by a fixed hard cap.
    pub async fn list_committed(&self) -> Result<Vec<CommittedSnapshot>, PlatformError> {
        let prefix = format!(
            "{}{SNAPSHOT_LAYOUT}/{}/",
            self.client.prefix(),
            self.platform_id
        );
        let mut token = None;
        let mut snapshots = Vec::new();
        loop {
            let mut request = self
                .client
                .inner()
                .list_objects_v2()
                .bucket(self.client.bucket())
                .prefix(&prefix)
                .max_keys(1000);
            if let Some(value) = &token {
                request = request.continuation_token(value);
            }
            let output = request
                .send()
                .await
                .map_err(|failure| error::from_list(&failure))?;
            for object in output.contents() {
                let Some(key) = object.key() else { continue };
                let Some(snapshot_id) = parse_manifest_key(&prefix, key) else {
                    continue;
                };
                snapshots.push(CommittedSnapshot {
                    snapshot_id,
                    manifest_key: key.to_owned(),
                });
                if snapshots.len() > MAX_LISTED_SNAPSHOTS {
                    return Err(snapshot_invalid());
                }
            }
            if output.is_truncated() == Some(true) {
                token = output.next_continuation_token().map(ToOwned::to_owned);
                if token.is_none() {
                    return Err(snapshot_invalid());
                }
            } else {
                break;
            }
        }
        snapshots.sort_by(|left, right| left.snapshot_id.cmp(&right.snapshot_id));
        Ok(snapshots)
    }

    /// Delete only old, exact-layout object prefixes that have no manifest commit marker.
    ///
    /// Unknown keys, malformed layouts, missing timestamps, and committed prefixes are
    /// deliberately retained for operator inspection.
    pub async fn cleanup_incomplete(
        &self,
        grace_deadline: SystemTime,
    ) -> Result<IncompleteSnapshotCleanup, PlatformError> {
        let prefix = format!(
            "{}{SNAPSHOT_LAYOUT}/{}/",
            self.client.prefix(),
            self.platform_id
        );
        let mut token = None;
        let mut prefixes: BTreeMap<String, IncompletePrefix> = BTreeMap::new();
        let mut listed = 0_usize;
        loop {
            let mut request = self
                .client
                .inner()
                .list_objects_v2()
                .bucket(self.client.bucket())
                .prefix(&prefix)
                .max_keys(1000);
            if let Some(value) = &token {
                request = request.continuation_token(value);
            }
            let output = request
                .send()
                .await
                .map_err(|failure| error::from_list(&failure))?;
            for object in output.contents() {
                listed = listed.checked_add(1).ok_or_else(snapshot_invalid)?;
                if listed > MAX_LISTED_SNAPSHOT_OBJECTS {
                    return Err(snapshot_invalid());
                }
                let Some(key) = object.key() else { continue };
                let Some(relative) = key.strip_prefix(&prefix) else {
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
                } else if parse_file_key(&prefix, key).as_deref() == Some(snapshot_id) {
                    let size = u64::try_from(object.size().unwrap_or(-1)).unwrap_or(u64::MAX);
                    let modified = object.last_modified().and_then(|timestamp| {
                        u64::try_from(timestamp.secs()).ok().and_then(|seconds| {
                            SystemTime::UNIX_EPOCH.checked_add(Duration::from_secs(seconds))
                        })
                    });
                    entry.objects.push((key.to_owned(), size, modified));
                } else {
                    entry.invalid = true;
                }
            }
            if output.is_truncated() == Some(true) {
                token = output.next_continuation_token().map(ToOwned::to_owned);
                if token.is_none() {
                    return Err(snapshot_invalid());
                }
            } else {
                break;
            }
        }

        let mut cleanup = IncompleteSnapshotCleanup::default();
        for entry in prefixes.into_values() {
            if entry.committed
                || entry.invalid
                || entry.objects.is_empty()
                || entry.objects.iter().any(|(_, size, modified)| {
                    *size == u64::MAX || modified.is_none_or(|value| value > grace_deadline)
                })
            {
                continue;
            }
            for (key, size, _) in entry.objects {
                self.delete_exact(&key).await?;
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
        self.client
            .inner()
            .delete_object()
            .bucket(self.client.bucket())
            .key(key)
            .send()
            .await
            .map_err(|failure| error::from_delete(&failure))?;
        Ok(())
    }

    fn validate_file_key(&self, key: &str) -> Result<(), PlatformError> {
        let prefix = format!(
            "{}{SNAPSHOT_LAYOUT}/{}/",
            self.client.prefix(),
            self.platform_id
        );
        parse_file_key(&prefix, key)
            .map(|_| ())
            .ok_or_else(snapshot_invalid)
    }

    fn validate_manifest_key(&self, key: &str) -> Result<(), PlatformError> {
        let prefix = format!(
            "{}{SNAPSHOT_LAYOUT}/{}/",
            self.client.prefix(),
            self.platform_id
        );
        parse_manifest_key(&prefix, key)
            .map(|_| ())
            .ok_or_else(snapshot_invalid)
    }
}

fn parse_manifest_key(prefix: &str, key: &str) -> Option<String> {
    let relative = key.strip_prefix(prefix)?;
    let snapshot_id = relative.strip_suffix("/manifest.json")?;
    (validate_snapshot_id(snapshot_id).is_ok()).then(|| snapshot_id.to_owned())
}

fn parse_file_key(prefix: &str, key: &str) -> Option<String> {
    let relative = key.strip_prefix(prefix)?;
    let (snapshot_id, object) = relative.split_once("/objects/")?;
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
        "platform snapshot S3 object is invalid",
    )
}
