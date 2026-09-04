//! Immutable content-addressed artifact store.

use crate::artifact::{ArtifactRef, parse_physical_key, parse_sha256, physical_key};
use crate::client::S3ArtifactClient;
use crate::error::{self, S3Stage};
use aws_sdk_s3::error::SdkError;
use aws_sdk_s3::primitives::ByteStream;
use aws_smithy_types::byte_stream::Length;
use bytes::Bytes;
use futures::Stream;
use futures::StreamExt;
use open_compute_core::{ErrorCode, PlatformError};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::io::{Read, Seek};
use std::path::Path;
use std::pin::pin;
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tokio::sync::{OwnedRwLockReadGuard, OwnedRwLockWriteGuard, RwLock};

const META_SHA256: &str = "sha256";
const KV_BACKUP_PREFIX: &str = "backups/kv/";
const D1_BACKUP_PREFIX: &str = "backups/d1/";

#[derive(Clone, Copy)]
enum BackupKind {
    Kv,
    D1,
}

impl BackupKind {
    const fn prefix(self) -> &'static str {
        match self {
            Self::Kv => KV_BACKUP_PREFIX,
            Self::D1 => D1_BACKUP_PREFIX,
        }
    }

    const fn key_error(self) -> &'static str {
        match self {
            Self::Kv => "KV backup object key is outside the system prefix",
            Self::D1 => "D1 backup object key is outside the system prefix",
        }
    }

    const fn size_error(self) -> &'static str {
        match self {
            Self::Kv => "KV backup exceeds the configured object limit",
            Self::D1 => "D1 backup exceeds the configured object limit",
        }
    }

    const fn staging_error(self) -> &'static str {
        match self {
            Self::Kv => "KV backup staging file is unavailable",
            Self::D1 => "D1 backup staging file is unavailable",
        }
    }

    const fn manifest_size_error(self) -> &'static str {
        match self {
            Self::Kv => "KV backup manifest is outside the fixed size limit",
            Self::D1 => "D1 backup manifest is outside the fixed size limit",
        }
    }

    const fn canonical_error(self) -> &'static str {
        match self {
            Self::Kv => "KV backup data object key is not canonical",
            Self::D1 => "D1 backup data object key is not canonical",
        }
    }
}

/// Remote object listed under the internal artifact prefix.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArtifactCandidate {
    /// Typed ref reconstructed from the internal key and remote size.
    pub artifact: ArtifactRef,
    /// Remote last-modified time when the service provided one.
    pub last_modified: Option<SystemTime>,
}

/// Immutable S3-backed artifact store.
#[derive(Debug, Clone)]
pub struct ArtifactStore {
    client: S3ArtifactClient,
    version_gc_gate: Arc<RwLock<()>>,
}

/// Read-side reservation held from version upload through authority commit.
pub struct ArtifactVersionReservation {
    _guard: OwnedRwLockReadGuard<()>,
}

impl std::fmt::Debug for ArtifactVersionReservation {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ArtifactVersionReservation")
            .finish_non_exhaustive()
    }
}

/// Exclusive fence held from the final authority snapshot through remote deletion.
pub struct ArtifactGcFence {
    _guard: OwnedRwLockWriteGuard<()>,
}

impl std::fmt::Debug for ArtifactGcFence {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ArtifactGcFence")
            .finish_non_exhaustive()
    }
}

impl ArtifactStore {
    /// Wrap a configured production client.
    #[must_use]
    pub fn new(client: S3ArtifactClient) -> Self {
        Self {
            client,
            version_gc_gate: Arc::new(RwLock::new(())),
        }
    }

    /// Fence artifact GC while a version uploads and commits its authoritative reference.
    pub async fn reserve_version_artifact(&self) -> ArtifactVersionReservation {
        ArtifactVersionReservation {
            _guard: self.version_gc_gate.clone().read_owned().await,
        }
    }

    /// Fence new version uploads while GC takes its final reference snapshot and deletes.
    pub async fn fence_version_gc(&self) -> ArtifactGcFence {
        ArtifactGcFence {
            _guard: self.version_gc_gate.clone().write_owned().await,
        }
    }

    /// Construct a host-owned KV backup key below the configured system prefix.
    pub fn kv_backup_key(&self, relative: &str) -> Result<String, PlatformError> {
        self.backup_key(BackupKind::Kv, relative)
    }

    /// Construct a host-owned D1 backup key below the configured system prefix.
    pub fn d1_backup_key(&self, relative: &str) -> Result<String, PlatformError> {
        self.backup_key(BackupKind::D1, relative)
    }

    /// Upload one verified KV backup file to its immutable host-generated key.
    pub async fn put_kv_backup_file(
        &self,
        relative: &str,
        path: &Path,
        expected_sha256: &str,
        expected_size: u64,
    ) -> Result<String, PlatformError> {
        self.put_backup_file(
            BackupKind::Kv,
            relative,
            path,
            expected_sha256,
            expected_size,
        )
        .await
    }

    /// Upload one verified D1 backup file to its immutable host-generated key.
    pub async fn put_d1_backup_file(
        &self,
        relative: &str,
        path: &Path,
        expected_sha256: &str,
        expected_size: u64,
    ) -> Result<String, PlatformError> {
        self.put_backup_file(
            BackupKind::D1,
            relative,
            path,
            expected_sha256,
            expected_size,
        )
        .await
    }

    async fn put_backup_file(
        &self,
        kind: BackupKind,
        relative: &str,
        path: &Path,
        expected_sha256: &str,
        expected_size: u64,
    ) -> Result<String, PlatformError> {
        let expected = parse_sha256(expected_sha256)?;
        if expected_size > self.client.max_artifact_bytes() {
            return Err(PlatformError::new(
                ErrorCode::LimitInvalid,
                kind.size_error(),
            ));
        }
        let key = self.backup_key(kind, relative)?;
        let mut file = std::fs::File::open(path)
            .map_err(|_| PlatformError::new(ErrorCode::DiskHardLimit, kind.staging_error()))?;
        let metadata = file.metadata().map_err(|_| error::integrity_error())?;
        if !metadata.file_type().is_file() || metadata.len() != expected_size {
            return Err(error::integrity_error());
        }
        let mut hasher = Sha256::new();
        let mut buffer = [0_u8; 64 * 1024];
        let mut total = 0_u64;
        loop {
            let count = file
                .read(&mut buffer)
                .map_err(|_| error::integrity_error())?;
            if count == 0 {
                break;
            }
            total = total.saturating_add(count as u64);
            if total > expected_size {
                return Err(error::integrity_error());
            }
            hasher.update(&buffer[..count]);
        }
        if total != expected_size || hasher.finalize().as_slice() != expected {
            return Err(error::integrity_error());
        }
        file.rewind().map_err(|_| error::integrity_error())?;
        let body = ByteStream::read_from()
            .file(tokio::fs::File::from_std(file))
            .length(Length::Exact(expected_size))
            .buffer_size(64 * 1024)
            .build()
            .await
            .map_err(|_| error::integrity_error())?;
        let put = self
            .client
            .inner()
            .put_object()
            .bucket(self.client.bucket())
            .key(&key)
            .body(body)
            .content_length(expected_size as i64)
            .metadata(META_SHA256, expected_sha256)
            .if_none_match("*")
            .send()
            .await;
        if let Err(err) = put {
            if let SdkError::ServiceError(service) = &err
                && matches!(service.raw().status().as_u16(), 409 | 412)
            {
                self.verify_backup_head(&key, expected_sha256, expected_size)
                    .await?;
                self.download_backup(&key, expected_sha256, expected_size, &mut std::io::sink())
                    .await?;
                return Ok(key);
            }
            let original = error::from_put(&err);
            if self
                .verify_backup_head(&key, expected_sha256, expected_size)
                .await
                .is_ok()
                && self
                    .download_backup(&key, expected_sha256, expected_size, &mut std::io::sink())
                    .await
                    .is_ok()
            {
                return Ok(key);
            }
            return Err(original);
        }
        self.verify_backup_head(&key, expected_sha256, expected_size)
            .await?;
        Ok(key)
    }

    /// Upload one small immutable JSON manifest below the KV backup prefix.
    pub async fn put_kv_backup_manifest(
        &self,
        relative: &str,
        bytes: Bytes,
    ) -> Result<String, PlatformError> {
        self.put_backup_manifest(BackupKind::Kv, relative, bytes)
            .await
    }

    /// Upload one small immutable JSON manifest below the D1 backup prefix.
    pub async fn put_d1_backup_manifest(
        &self,
        relative: &str,
        bytes: Bytes,
    ) -> Result<String, PlatformError> {
        self.put_backup_manifest(BackupKind::D1, relative, bytes)
            .await
    }

    async fn put_backup_manifest(
        &self,
        kind: BackupKind,
        relative: &str,
        bytes: Bytes,
    ) -> Result<String, PlatformError> {
        if bytes.is_empty() || bytes.len() > 64 * 1024 {
            return Err(PlatformError::new(
                ErrorCode::LimitInvalid,
                kind.manifest_size_error(),
            ));
        }
        let key = self.backup_key(kind, relative)?;
        let digest = hex::encode(Sha256::digest(&bytes));
        let size = bytes.len() as u64;
        let put = self
            .client
            .inner()
            .put_object()
            .bucket(self.client.bucket())
            .key(&key)
            .body(ByteStream::from(bytes.clone()))
            .content_length(size as i64)
            .content_type("application/json")
            .metadata(META_SHA256, &digest)
            .if_none_match("*")
            .send()
            .await;
        if let Err(err) = put {
            let conflict = matches!(
                &err,
                SdkError::ServiceError(service)
                    if matches!(service.raw().status().as_u16(), 409 | 412)
            );
            if !conflict {
                let original = error::from_put(&err);
                let reconciled = self.verify_backup_head(&key, &digest, size).await.is_ok()
                    && self
                        .get_backup_manifest(&key)
                        .await
                        .is_ok_and(|stored| stored == bytes);
                if !reconciled {
                    return Err(original);
                }
            }
        }
        self.verify_backup_head(&key, &digest, size).await?;
        let stored = self.get_backup_manifest(&key).await?;
        if stored != bytes {
            return Err(error::integrity_error());
        }
        Ok(key)
    }

    /// Download a small manifest while verifying its declared digest and size.
    pub async fn get_kv_backup_manifest(&self, key: &str) -> Result<Bytes, PlatformError> {
        self.validate_backup_key(BackupKind::Kv, key)?;
        self.get_backup_manifest(key).await
    }

    /// Download a small D1 manifest while verifying its declared digest and size.
    pub async fn get_d1_backup_manifest(&self, key: &str) -> Result<Bytes, PlatformError> {
        self.validate_backup_key(BackupKind::D1, key)?;
        self.get_backup_manifest(key).await
    }

    async fn get_backup_manifest(&self, key: &str) -> Result<Bytes, PlatformError> {
        let head = self
            .client
            .inner()
            .head_object()
            .bucket(self.client.bucket())
            .key(key)
            .send()
            .await
            .map_err(|err| error::from_head(&err))?;
        let size = u64::try_from(head.content_length().unwrap_or(-1)).unwrap_or(u64::MAX);
        if size == 0 || size > 64 * 1024 {
            return Err(error::integrity_error());
        }
        let digest = head
            .metadata()
            .and_then(|metadata| metadata.get(META_SHA256))
            .ok_or_else(error::integrity_error)?;
        let expected = parse_sha256(digest)?;
        let got = self
            .client
            .inner()
            .get_object()
            .bucket(self.client.bucket())
            .key(key)
            .send()
            .await
            .map_err(|err| error::from_get(&err))?;
        let mut body = pin!(got.body);
        let mut bytes = Vec::with_capacity(size as usize);
        let mut hasher = Sha256::new();
        while let Some(chunk) = body.next().await {
            let chunk = chunk.map_err(|_| error::unavailable(S3Stage::Server))?;
            if bytes.len().saturating_add(chunk.len()) > size as usize {
                return Err(error::integrity_error());
            }
            hasher.update(&chunk);
            bytes.extend_from_slice(&chunk);
        }
        if bytes.len() != size as usize || hasher.finalize().as_slice() != expected {
            return Err(error::integrity_error());
        }
        Ok(Bytes::from(bytes))
    }

    /// Derive the sibling manifest object from a persisted data object key.
    pub fn kv_backup_manifest_key(&self, data_key: &str) -> Result<String, PlatformError> {
        self.backup_manifest_key(BackupKind::Kv, data_key)
    }

    /// Derive the sibling manifest object from a persisted D1 data object key.
    pub fn d1_backup_manifest_key(&self, data_key: &str) -> Result<String, PlatformError> {
        self.backup_manifest_key(BackupKind::D1, data_key)
    }

    /// Download and verify one persisted host-owned KV backup object.
    pub async fn download_kv_backup<W: std::io::Write>(
        &self,
        key: &str,
        expected_sha256: &str,
        expected_size: u64,
        writer: &mut W,
    ) -> Result<(), PlatformError> {
        self.validate_backup_key(BackupKind::Kv, key)?;
        self.download_backup(key, expected_sha256, expected_size, writer)
            .await
    }

    /// Download and verify one persisted host-owned D1 backup object.
    pub async fn download_d1_backup<W: std::io::Write>(
        &self,
        key: &str,
        expected_sha256: &str,
        expected_size: u64,
        writer: &mut W,
    ) -> Result<(), PlatformError> {
        self.validate_backup_key(BackupKind::D1, key)?;
        self.download_backup(key, expected_sha256, expected_size, writer)
            .await
    }

    async fn download_backup<W: std::io::Write>(
        &self,
        key: &str,
        expected_sha256: &str,
        expected_size: u64,
        writer: &mut W,
    ) -> Result<(), PlatformError> {
        let expected = parse_sha256(expected_sha256)?;
        self.verify_backup_head(key, expected_sha256, expected_size)
            .await?;
        let got = self
            .client
            .inner()
            .get_object()
            .bucket(self.client.bucket())
            .key(key)
            .send()
            .await
            .map_err(|err| error::from_get(&err))?;
        let mut body = pin!(got.body);
        let mut hasher = Sha256::new();
        let mut total = 0_u64;
        while let Some(chunk) = body.next().await {
            let chunk = chunk.map_err(|_| error::unavailable(S3Stage::Server))?;
            total = total.saturating_add(chunk.len() as u64);
            if total > expected_size {
                return Err(error::integrity_error());
            }
            hasher.update(&chunk);
            writer.write_all(&chunk).map_err(|_| {
                PlatformError::new(ErrorCode::DiskHardLimit, "failed to stage backup")
            })?;
        }
        if total != expected_size || hasher.finalize().as_slice() != expected {
            return Err(error::integrity_error());
        }
        Ok(())
    }

    /// Delete one exact host-owned KV backup object.
    pub async fn delete_kv_backup(&self, key: &str) -> Result<(), PlatformError> {
        self.validate_backup_key(BackupKind::Kv, key)?;
        self.delete_backup(key).await
    }

    /// Delete one exact host-owned D1 backup object.
    pub async fn delete_d1_backup(&self, key: &str) -> Result<(), PlatformError> {
        self.validate_backup_key(BackupKind::D1, key)?;
        self.delete_backup(key).await
    }

    async fn delete_backup(&self, key: &str) -> Result<(), PlatformError> {
        self.client
            .inner()
            .delete_object()
            .bucket(self.client.bucket())
            .key(key)
            .send()
            .await
            .map_err(|err| error::from_delete(&err))?;
        Ok(())
    }

    async fn verify_backup_head(
        &self,
        key: &str,
        expected_sha256: &str,
        expected_size: u64,
    ) -> Result<(), PlatformError> {
        let head = self
            .client
            .inner()
            .head_object()
            .bucket(self.client.bucket())
            .key(key)
            .send()
            .await
            .map_err(|err| error::from_head(&err))?;
        let size = u64::try_from(head.content_length().unwrap_or(-1)).unwrap_or(u64::MAX);
        let digest = head
            .metadata()
            .and_then(|metadata| metadata.get(META_SHA256));
        if size != expected_size || digest.is_none_or(|value| value != expected_sha256) {
            return Err(error::integrity_error());
        }
        Ok(())
    }

    fn backup_key(&self, kind: BackupKind, relative: &str) -> Result<String, PlatformError> {
        if relative.is_empty()
            || relative.contains("..")
            || relative.starts_with('/')
            || !relative.starts_with(kind.prefix())
        {
            return Err(PlatformError::new(
                ErrorCode::ConfigInvalid,
                kind.key_error(),
            ));
        }
        Ok(format!("{}{relative}", self.client.prefix()))
    }

    fn validate_backup_key(&self, kind: BackupKind, key: &str) -> Result<(), PlatformError> {
        let prefix = format!("{}{}", self.client.prefix(), kind.prefix());
        if !key.starts_with(&prefix) || key.contains("..") {
            return Err(PlatformError::new(
                ErrorCode::ConfigInvalid,
                kind.key_error(),
            ));
        }
        Ok(())
    }

    fn backup_manifest_key(
        &self,
        kind: BackupKind,
        data_key: &str,
    ) -> Result<String, PlatformError> {
        self.validate_backup_key(kind, data_key)?;
        data_key
            .strip_suffix("/data.sqlite")
            .map(|prefix| format!("{prefix}/manifest.json"))
            .ok_or_else(|| PlatformError::new(ErrorCode::ConfigInvalid, kind.canonical_error()))
    }

    /// Stream bytes to S3, verifying digest and size before success.
    pub async fn put_verified<S, E>(
        &self,
        stream: S,
        expected_sha256: &str,
        expected_size: u64,
    ) -> Result<ArtifactRef, PlatformError>
    where
        S: Stream<Item = Result<Bytes, E>> + Send,
        E: std::error::Error + Send + Sync + 'static,
    {
        let expected = parse_sha256(expected_sha256)?;
        if expected_size > self.client.max_artifact_bytes() {
            return Err(PlatformError::new(
                ErrorCode::LimitInvalid,
                "artifact exceeds configured maximum size",
            ));
        }
        let mut hasher = Sha256::new();
        let mut buf = Vec::with_capacity(usize::try_from(expected_size).unwrap_or(0));
        let mut stream = pin!(stream);
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|_| {
                PlatformError::new(ErrorCode::S3Unavailable, "artifact stream read failed")
            })?;
            let next = buf.len() as u64 + chunk.len() as u64;
            if next > self.client.max_artifact_bytes() || next > expected_size {
                return Err(PlatformError::new(
                    ErrorCode::LimitInvalid,
                    "artifact exceeds configured maximum size",
                ));
            }
            hasher.update(&chunk);
            buf.extend_from_slice(&chunk);
        }
        if buf.len() as u64 != expected_size {
            return Err(error::integrity_error());
        }
        let actual = hasher.finalize();
        if actual.as_slice() != expected {
            return Err(error::integrity_error());
        }
        let hex_digest = hex::encode(expected);
        let key = physical_key(self.client.prefix(), &hex_digest);
        let artifact = ArtifactRef::new(1, &hex_digest, expected_size)?;

        match self.head(&artifact).await {
            Ok(existing) => {
                self.download_verified(&existing, &mut std::io::sink())
                    .await?;
                return Ok(existing);
            }
            Err(err) if error::is_not_found(&err) => {}
            Err(err) => return Err(err),
        }

        let put = self
            .client
            .inner()
            .put_object()
            .bucket(self.client.bucket())
            .key(&key)
            .body(ByteStream::from(buf))
            .content_length(expected_size as i64)
            .metadata(META_SHA256, &hex_digest)
            .if_none_match("*")
            .send()
            .await;
        if let Err(err) = put {
            if let SdkError::ServiceError(svc) = &err {
                let status = svc.raw().status().as_u16();
                if status == 412 || status == 409 {
                    let existing = self.head(&artifact).await?;
                    self.download_verified(&existing, &mut std::io::sink())
                        .await?;
                    return Ok(existing);
                }
            }
            return Err(error::from_put(&err));
        }

        let verified = self.head(&artifact).await?;
        self.download_verified(&verified, &mut std::io::sink())
            .await?;
        Ok(verified)
    }

    /// Upload a private regular file without buffering the artifact in heap memory.
    ///
    /// The already-opened file is hashed and sized before it becomes the request
    /// body, preventing a pathname replacement between verification and upload.
    pub async fn put_verified_file(
        &self,
        path: &Path,
        expected_sha256: &str,
        expected_size: u64,
    ) -> Result<ArtifactRef, PlatformError> {
        let expected = parse_sha256(expected_sha256)?;
        if expected_size > self.client.max_artifact_bytes() {
            return Err(PlatformError::new(
                ErrorCode::LimitInvalid,
                "artifact exceeds configured maximum size",
            ));
        }
        let mut file = std::fs::File::open(path).map_err(|_| {
            PlatformError::new(
                ErrorCode::DiskHardLimit,
                "artifact staging file is unavailable",
            )
        })?;
        let metadata = file.metadata().map_err(|_| {
            PlatformError::new(
                ErrorCode::DiskHardLimit,
                "artifact staging file is unavailable",
            )
        })?;
        if !metadata.file_type().is_file() || metadata.len() != expected_size {
            return Err(error::integrity_error());
        }
        let mut hasher = Sha256::new();
        let mut read = 0_u64;
        let mut buffer = vec![0_u8; 64 * 1024];
        loop {
            let count = file.read(&mut buffer).map_err(|_| {
                PlatformError::new(
                    ErrorCode::DiskHardLimit,
                    "failed to read artifact staging file",
                )
            })?;
            if count == 0 {
                break;
            }
            read = read
                .checked_add(u64::try_from(count).map_err(|_| error::integrity_error())?)
                .ok_or_else(error::integrity_error)?;
            if read > expected_size {
                return Err(error::integrity_error());
            }
            hasher.update(&buffer[..count]);
        }
        if read != expected_size || hasher.finalize().as_slice() != expected {
            return Err(error::integrity_error());
        }
        file.rewind().map_err(|_| {
            PlatformError::new(
                ErrorCode::DiskHardLimit,
                "failed to rewind artifact staging file",
            )
        })?;

        let hex_digest = hex::encode(expected);
        let key = physical_key(self.client.prefix(), &hex_digest);
        let artifact = ArtifactRef::new(1, &hex_digest, expected_size)?;
        match self.head(&artifact).await {
            Ok(existing) => {
                self.download_verified(&existing, &mut std::io::sink())
                    .await?;
                return Ok(existing);
            }
            Err(err) if error::is_not_found(&err) => {}
            Err(err) => return Err(err),
        }

        let body = ByteStream::read_from()
            .file(tokio::fs::File::from_std(file))
            .length(Length::Exact(expected_size))
            .buffer_size(64 * 1024)
            .build()
            .await
            .map_err(|_| {
                PlatformError::new(
                    ErrorCode::DiskHardLimit,
                    "failed to stream artifact staging file",
                )
            })?;
        let put = self
            .client
            .inner()
            .put_object()
            .bucket(self.client.bucket())
            .key(&key)
            .body(body)
            .content_length(expected_size as i64)
            .metadata(META_SHA256, &hex_digest)
            .if_none_match("*")
            .send()
            .await;
        if let Err(err) = put {
            if let SdkError::ServiceError(svc) = &err {
                let status = svc.raw().status().as_u16();
                if status == 412 || status == 409 {
                    let existing = self.head(&artifact).await?;
                    self.download_verified(&existing, &mut std::io::sink())
                        .await?;
                    return Ok(existing);
                }
            }
            return Err(error::from_put(&err));
        }
        let verified = self.head(&artifact).await?;
        self.download_verified(&verified, &mut std::io::sink())
            .await?;
        Ok(verified)
    }

    /// HEAD the object and verify declared size/metadata.
    pub async fn head(&self, artifact: &ArtifactRef) -> Result<ArtifactRef, PlatformError> {
        let key = artifact.physical_key(self.client.prefix());
        let head = self
            .client
            .inner()
            .head_object()
            .bucket(self.client.bucket())
            .key(&key)
            .send()
            .await
            .map_err(|err| error::from_head(&err))?;
        let len = u64::try_from(head.content_length().unwrap_or(-1)).unwrap_or(u64::MAX);
        if len != artifact.size() {
            return Err(error::integrity_error());
        }
        if let Some(meta) = head.metadata().and_then(|m| m.get(META_SHA256))
            && meta != &artifact.sha256_hex()
        {
            return Err(error::integrity_error());
        }
        Ok(artifact.clone())
    }

    /// Stream a GET into `writer`, hashing and bounding size incrementally.
    ///
    /// The object is verified before this method returns. `writer` receives
    /// chunks as they arrive; callers that persist to disk must fsync and
    /// publish only after success. This is not a lazily verified stream.
    pub async fn download_verified<W: std::io::Write>(
        &self,
        artifact: &ArtifactRef,
        writer: &mut W,
    ) -> Result<(), PlatformError> {
        if artifact.size() > self.client.max_artifact_bytes() {
            return Err(PlatformError::new(
                ErrorCode::LimitInvalid,
                "artifact exceeds configured maximum size",
            ));
        }
        let key = artifact.physical_key(self.client.prefix());
        let got = self
            .client
            .inner()
            .get_object()
            .bucket(self.client.bucket())
            .key(&key)
            .send()
            .await
            .map_err(|err| error::from_get(&err))?;
        let mut hasher = Sha256::new();
        let mut written = 0_u64;
        let mut body = pin!(got.body);
        while let Some(chunk) = body.next().await {
            let chunk = chunk.map_err(|_| error::unavailable(S3Stage::Server))?;
            let next = written.saturating_add(chunk.len() as u64);
            if next > self.client.max_artifact_bytes() {
                return Err(PlatformError::new(
                    ErrorCode::LimitInvalid,
                    "artifact exceeds configured maximum size",
                ));
            }
            if next > artifact.size() {
                return Err(error::integrity_error());
            }
            hasher.update(chunk.as_ref());
            writer.write_all(chunk.as_ref()).map_err(|_| {
                PlatformError::new(
                    ErrorCode::DiskHardLimit,
                    "failed to write verified artifact bytes",
                )
            })?;
            written = next;
        }
        if written != artifact.size() {
            return Err(error::integrity_error());
        }
        if hasher.finalize().as_slice() != artifact.sha256_bytes() {
            return Err(error::integrity_error());
        }
        Ok(())
    }

    /// Buffer a fully verified object. This convenience helper is not a stream.
    pub async fn open(&self, artifact: &ArtifactRef) -> Result<Bytes, PlatformError> {
        let mut buf = Vec::new();
        self.download_verified(artifact, &mut buf).await?;
        Ok(Bytes::from(buf))
    }

    /// Delete an object only when the caller supplies a validated internal ref.
    pub async fn delete_unreferenced(&self, artifact: &ArtifactRef) -> Result<(), PlatformError> {
        let key = artifact.physical_key(self.client.prefix());
        self.client
            .inner()
            .delete_object()
            .bucket(self.client.bucket())
            .key(&key)
            .send()
            .await
            .map_err(|err| error::from_delete(&err))?;
        Ok(())
    }

    /// Bounded listing of internal artifact candidates for grace-period GC.
    pub async fn list_candidates(&self) -> Result<Vec<ArtifactCandidate>, PlatformError> {
        let prefix = format!("{}artifacts/v1/sha256/", self.client.prefix());
        let mut out = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut req = self
                .client
                .inner()
                .list_objects_v2()
                .bucket(self.client.bucket())
                .prefix(&prefix)
                .max_keys(1000);
            if let Some(t) = &token {
                req = req.continuation_token(t);
            }
            let resp = req.send().await.map_err(|err| error::from_list(&err))?;
            for obj in resp.contents() {
                let Some(key) = obj.key() else {
                    continue;
                };
                let Ok(digest) = parse_physical_key(self.client.prefix(), key) else {
                    continue;
                };
                let size = u64::try_from(obj.size().unwrap_or(0)).unwrap_or(0);
                let Ok(artifact) = ArtifactRef::new(1, &digest, size) else {
                    continue;
                };
                let last_modified = obj.last_modified().and_then(|ts| {
                    SystemTime::UNIX_EPOCH.checked_add(Duration::from_secs(ts.secs() as u64))
                });
                out.push(ArtifactCandidate {
                    artifact,
                    last_modified,
                });
            }
            if resp.is_truncated() == Some(true) {
                token = resp.next_continuation_token().map(ToOwned::to_owned);
                if token.is_none() {
                    break;
                }
            } else {
                break;
            }
        }
        Ok(out)
    }

    /// Delete verified unreferenced remote candidates older than `grace_deadline`.
    pub async fn gc_unreferenced(
        &self,
        _fence: &ArtifactGcFence,
        referenced: &HashSet<ArtifactRef>,
        grace_deadline: SystemTime,
    ) -> Result<u64, PlatformError> {
        let mut deleted = 0_u64;
        for candidate in self.list_candidates().await? {
            if referenced.contains(&candidate.artifact) {
                continue;
            }
            let Some(modified) = candidate.last_modified else {
                continue;
            };
            if modified > grace_deadline {
                continue;
            }
            self.delete_unreferenced(&candidate.artifact).await?;
            deleted += 1;
        }
        Ok(deleted)
    }
}

#[cfg(test)]
#[path = "store_tests.rs"]
mod tests;
