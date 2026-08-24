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
use std::time::{Duration, SystemTime};

const META_SHA256: &str = "sha256";

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
}

impl ArtifactStore {
    /// Wrap a configured production client.
    #[must_use]
    pub fn new(client: S3ArtifactClient) -> Self {
        Self { client }
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
