//! Immutable content-addressed artifact and backup store.

use crate::artifact::{ArtifactRef, parse_physical_key, parse_sha256, physical_key};
use crate::backend::{
    BackendError, GetOptions, HeadOptions, ObjectBackend, ObjectHttpMetadata, ObjectKey,
    ObjectMetadata, ObjectSource, PutMode, PutOptions, open_private_source,
};
use crate::error;
use bytes::Bytes;
use futures::{Stream, StreamExt as _};
use open_compute_core::{ErrorCode, PlatformError};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashSet};
use std::io::{Read as _, Seek as _};
use std::path::Path;
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

/// Object listed under the internal artifact prefix.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArtifactCandidate {
    /// Typed ref reconstructed from the internal key and stored size.
    pub artifact: ArtifactRef,
    /// Commit time when the backend provided one.
    pub last_modified: Option<SystemTime>,
}

/// Immutable artifact store backed by the selected object authority.
#[derive(Debug, Clone)]
pub struct ArtifactStore {
    backend: ObjectBackend,
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

/// Exclusive fence held from the final authority snapshot through object deletion.
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
    /// Wrap the selected production object backend.
    #[must_use]
    pub fn new(backend: ObjectBackend) -> Self {
        Self {
            backend,
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
        if expected_size > self.backend.max_object_bytes() {
            return Err(PlatformError::new(
                ErrorCode::LimitInvalid,
                kind.size_error(),
            ));
        }
        let key = self.backup_key(kind, relative)?;
        let mut file =
            open_private_source(path, expected_size).map_err(|failure| match failure {
                BackendError::Unavailable => {
                    PlatformError::new(ErrorCode::DiskHardLimit, kind.staging_error())
                }
                _ => error::integrity_error(),
            })?;
        verify_reader(&mut file, expected_size, &expected)?;
        file.rewind().map_err(|_| error::integrity_error())?;
        let result = self
            .backend
            .put(
                &object_key(&key)?,
                ObjectSource::File {
                    file,
                    length: expected_size,
                },
                immutable_options(expected_size, expected_sha256, None),
            )
            .await;
        self.reconcile_immutable_put(result, &key, expected_sha256, expected_size)
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
        let result = self
            .backend
            .put(
                &object_key(&key)?,
                ObjectSource::Bytes(bytes.clone()),
                immutable_options(size, &digest, Some("application/json")),
            )
            .await;
        if let Err(failure) = result
            && failure != BackendError::PreconditionFailed
        {
            let original = error::from_backend(failure);
            let reconciled = self.verify_backup_head(&key, &digest, size).await.is_ok()
                && self
                    .get_backup_manifest(&key)
                    .await
                    .is_ok_and(|stored| stored == bytes);
            if !reconciled {
                return Err(original);
            }
        }
        self.verify_backup_head(&key, &digest, size).await?;
        if self.get_backup_manifest(&key).await? != bytes {
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
        let key = object_key(key)?;
        let head = self
            .backend
            .head(&key, HeadOptions::default())
            .await
            .map_err(error::from_backend)?;
        if head.size == 0 || head.size > 64 * 1024 {
            return Err(error::integrity_error());
        }
        let digest = head
            .user
            .get(META_SHA256)
            .ok_or_else(error::integrity_error)?;
        let expected = parse_sha256(digest)?;
        let got = self
            .backend
            .get(&key, GetOptions::default())
            .await
            .map_err(error::from_backend)?;
        let mut body = std::pin::pin!(got.body);
        let mut bytes = Vec::with_capacity(head.size as usize);
        let mut hasher = Sha256::new();
        while let Some(chunk) = body.next().await {
            let chunk = chunk.map_err(|_| object_stream_error())?;
            if bytes.len().saturating_add(chunk.len()) > head.size as usize {
                return Err(error::integrity_error());
            }
            hasher.update(&chunk);
            bytes.extend_from_slice(&chunk);
        }
        if bytes.len() != head.size as usize || hasher.finalize().as_slice() != expected {
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
            .backend
            .get(&object_key(key)?, GetOptions::default())
            .await
            .map_err(error::from_backend)?;
        verify_body(got.body, expected_size, &expected, writer).await
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
        self.backend
            .delete(&object_key(key)?)
            .await
            .map_err(error::from_backend)
    }

    async fn verify_backup_head(
        &self,
        key: &str,
        expected_sha256: &str,
        expected_size: u64,
    ) -> Result<(), PlatformError> {
        let head = self
            .backend
            .head(&object_key(key)?, HeadOptions::default())
            .await
            .map_err(error::from_backend)?;
        if head.size != expected_size
            || head
                .user
                .get(META_SHA256)
                .is_none_or(|digest| digest != expected_sha256)
        {
            return Err(error::integrity_error());
        }
        Ok(())
    }

    async fn reconcile_immutable_put(
        &self,
        result: Result<ObjectMetadata, BackendError>,
        key: &str,
        expected_sha256: &str,
        expected_size: u64,
    ) -> Result<(), PlatformError> {
        if let Err(failure) = result {
            if failure == BackendError::PreconditionFailed {
                self.verify_backup_head(key, expected_sha256, expected_size)
                    .await?;
                self.download_backup(key, expected_sha256, expected_size, &mut std::io::sink())
                    .await?;
                return Ok(());
            }
            let original = error::from_backend(failure);
            if self
                .verify_backup_head(key, expected_sha256, expected_size)
                .await
                .is_ok()
                && self
                    .download_backup(key, expected_sha256, expected_size, &mut std::io::sink())
                    .await
                    .is_ok()
            {
                return Ok(());
            }
            return Err(original);
        }
        self.verify_backup_head(key, expected_sha256, expected_size)
            .await
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
        Ok(format!("{}{relative}", self.backend.prefix()))
    }

    fn validate_backup_key(&self, kind: BackupKind, key: &str) -> Result<(), PlatformError> {
        let prefix = format!("{}{}", self.backend.prefix(), kind.prefix());
        if !key.starts_with(&prefix) || key.contains("..") {
            return Err(PlatformError::new(
                ErrorCode::ConfigInvalid,
                kind.key_error(),
            ));
        }
        object_key(key)?;
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

    /// Stream bytes to object storage, verifying digest and size before success.
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
        if expected_size > self.backend.max_object_bytes() {
            return Err(artifact_too_large());
        }
        let mut hasher = Sha256::new();
        let mut bytes = Vec::with_capacity(usize::try_from(expected_size).unwrap_or(0));
        let mut stream = std::pin::pin!(stream);
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|_| object_stream_error())?;
            let next = bytes.len() as u64 + chunk.len() as u64;
            if next > self.backend.max_object_bytes() || next > expected_size {
                return Err(artifact_too_large());
            }
            hasher.update(&chunk);
            bytes.extend_from_slice(&chunk);
        }
        if bytes.len() as u64 != expected_size || hasher.finalize().as_slice() != expected {
            return Err(error::integrity_error());
        }
        let digest = hex::encode(expected);
        self.publish_artifact(
            ObjectSource::Bytes(Bytes::from(bytes)),
            &digest,
            expected_size,
        )
        .await
    }

    /// Upload a private regular file without buffering the artifact in heap memory.
    pub async fn put_verified_file(
        &self,
        path: &Path,
        expected_sha256: &str,
        expected_size: u64,
    ) -> Result<ArtifactRef, PlatformError> {
        let expected = parse_sha256(expected_sha256)?;
        if expected_size > self.backend.max_object_bytes() {
            return Err(artifact_too_large());
        }
        let mut file =
            open_private_source(path, expected_size).map_err(|failure| match failure {
                BackendError::Unavailable => PlatformError::new(
                    ErrorCode::DiskHardLimit,
                    "artifact staging file is unavailable",
                ),
                _ => error::integrity_error(),
            })?;
        verify_reader(&mut file, expected_size, &expected)?;
        file.rewind().map_err(|_| {
            PlatformError::new(
                ErrorCode::DiskHardLimit,
                "failed to rewind artifact staging file",
            )
        })?;
        let digest = hex::encode(expected);
        self.publish_artifact(
            ObjectSource::File {
                file,
                length: expected_size,
            },
            &digest,
            expected_size,
        )
        .await
    }

    async fn publish_artifact(
        &self,
        source: ObjectSource,
        digest: &str,
        size: u64,
    ) -> Result<ArtifactRef, PlatformError> {
        let key = physical_key(self.backend.prefix(), digest);
        let artifact = ArtifactRef::new(1, digest, size)?;
        match self.head(&artifact).await {
            Ok(existing) => {
                self.download_verified(&existing, &mut std::io::sink())
                    .await?;
                return Ok(existing);
            }
            Err(failure) if error::is_not_found(&failure) => {}
            Err(failure) => return Err(failure),
        }
        let result = self
            .backend
            .put(
                &object_key(&key)?,
                source,
                immutable_options(size, digest, None),
            )
            .await;
        if let Err(failure) = result
            && failure != BackendError::PreconditionFailed
        {
            return Err(error::from_backend(failure));
        }
        let verified = self.head(&artifact).await?;
        self.download_verified(&verified, &mut std::io::sink())
            .await?;
        Ok(verified)
    }

    /// HEAD the object and verify declared size and metadata.
    pub async fn head(&self, artifact: &ArtifactRef) -> Result<ArtifactRef, PlatformError> {
        let head = self
            .backend
            .head(
                &object_key(&artifact.physical_key(self.backend.prefix()))?,
                HeadOptions::default(),
            )
            .await
            .map_err(error::from_backend)?;
        if head.size != artifact.size()
            || head
                .user
                .get(META_SHA256)
                .is_some_and(|digest| digest != &artifact.sha256_hex())
        {
            return Err(error::integrity_error());
        }
        Ok(artifact.clone())
    }

    /// Stream a GET into `writer`, hashing and bounding size incrementally.
    pub async fn download_verified<W: std::io::Write>(
        &self,
        artifact: &ArtifactRef,
        writer: &mut W,
    ) -> Result<(), PlatformError> {
        if artifact.size() > self.backend.max_object_bytes() {
            return Err(artifact_too_large());
        }
        let got = self
            .backend
            .get(
                &object_key(&artifact.physical_key(self.backend.prefix()))?,
                GetOptions::default(),
            )
            .await
            .map_err(error::from_backend)?;
        if got.metadata.size > self.backend.max_object_bytes() {
            return Err(artifact_too_large());
        }
        if got.metadata.size != artifact.size() {
            return Err(error::integrity_error());
        }
        verify_body(got.body, artifact.size(), artifact.sha256_bytes(), writer).await
    }

    /// Buffer a fully verified object. This convenience helper is not a stream.
    pub async fn open(&self, artifact: &ArtifactRef) -> Result<Bytes, PlatformError> {
        let mut bytes = Vec::new();
        self.download_verified(artifact, &mut bytes).await?;
        Ok(Bytes::from(bytes))
    }

    /// Delete an object only when the caller supplies a validated internal ref.
    pub async fn delete_unreferenced(&self, artifact: &ArtifactRef) -> Result<(), PlatformError> {
        self.backend
            .delete(&object_key(&artifact.physical_key(self.backend.prefix()))?)
            .await
            .map_err(error::from_backend)
    }

    /// Bounded listing of internal artifact candidates for grace-period GC.
    pub async fn list_candidates(&self) -> Result<Vec<ArtifactCandidate>, PlatformError> {
        let prefix = format!("{}artifacts/v1/sha256/", self.backend.prefix());
        let mut output = Vec::new();
        let mut cursor = None;
        loop {
            let page = self
                .backend
                .list(&prefix, 1000, cursor.as_deref())
                .await
                .map_err(error::from_backend)?;
            for object in page.objects {
                let Ok(digest) = parse_physical_key(self.backend.prefix(), object.key.as_str())
                else {
                    continue;
                };
                let Ok(artifact) = ArtifactRef::new(1, &digest, object.metadata.size) else {
                    continue;
                };
                let last_modified = u64::try_from(object.metadata.last_modified_ms)
                    .ok()
                    .filter(|milliseconds| *milliseconds > 0)
                    .and_then(|milliseconds| {
                        SystemTime::UNIX_EPOCH.checked_add(Duration::from_millis(milliseconds))
                    });
                output.push(ArtifactCandidate {
                    artifact,
                    last_modified,
                });
            }
            cursor = page.next_cursor;
            if cursor.is_none() {
                break;
            }
        }
        Ok(output)
    }

    /// Delete verified unreferenced candidates older than `grace_deadline`.
    pub async fn gc_unreferenced(
        &self,
        _fence: &ArtifactGcFence,
        referenced: &HashSet<ArtifactRef>,
        grace_deadline: SystemTime,
    ) -> Result<u64, PlatformError> {
        let mut deleted = 0_u64;
        for candidate in self.list_candidates().await? {
            if referenced.contains(&candidate.artifact)
                || candidate
                    .last_modified
                    .is_none_or(|modified| modified > grace_deadline)
            {
                continue;
            }
            self.delete_unreferenced(&candidate.artifact).await?;
            deleted += 1;
        }
        Ok(deleted)
    }
}

fn object_key(key: &str) -> Result<ObjectKey, PlatformError> {
    ObjectKey::new(key.to_owned()).map_err(error::from_backend)
}

fn immutable_options(size: u64, sha256: &str, content_type: Option<&str>) -> PutOptions {
    let mut user = BTreeMap::new();
    user.insert(META_SHA256.to_owned(), sha256.to_owned());
    PutOptions {
        mode: PutMode::CreateOnly,
        metadata: ObjectMetadata {
            size,
            user,
            http: ObjectHttpMetadata {
                content_type: content_type.map(str::to_owned),
                ..ObjectHttpMetadata::default()
            },
            ..ObjectMetadata::default()
        },
        customer_key: None,
    }
}

fn verify_reader(
    reader: &mut std::fs::File,
    expected_size: u64,
    expected_sha256: &[u8; 32],
) -> Result<(), PlatformError> {
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    let mut total = 0_u64;
    loop {
        let count = reader
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
    if total != expected_size || hasher.finalize().as_slice() != expected_sha256 {
        return Err(error::integrity_error());
    }
    Ok(())
}

async fn verify_body<W: std::io::Write>(
    body: crate::ObjectBody,
    expected_size: u64,
    expected_sha256: &[u8],
    writer: &mut W,
) -> Result<(), PlatformError> {
    let mut body = std::pin::pin!(body);
    let mut hasher = Sha256::new();
    let mut total = 0_u64;
    while let Some(chunk) = body.next().await {
        let chunk = chunk.map_err(|_| object_stream_error())?;
        total = total.saturating_add(chunk.len() as u64);
        if total > expected_size {
            return Err(error::integrity_error());
        }
        hasher.update(&chunk);
        writer.write_all(&chunk).map_err(|_| {
            PlatformError::new(ErrorCode::DiskHardLimit, "failed to stage object bytes")
        })?;
    }
    if total != expected_size || hasher.finalize().as_slice() != expected_sha256 {
        return Err(error::integrity_error());
    }
    Ok(())
}

const fn artifact_too_large() -> PlatformError {
    PlatformError::new(
        ErrorCode::LimitInvalid,
        "artifact exceeds configured maximum size",
    )
}

const fn object_stream_error() -> PlatformError {
    PlatformError::new(
        ErrorCode::ObjectStorageUnavailable,
        "object storage body stream failed",
    )
}

#[cfg(test)]
#[path = "store_tests.rs"]
mod tests;
