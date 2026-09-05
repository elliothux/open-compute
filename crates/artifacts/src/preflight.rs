//! Startup preflight for the selected object-byte authority.

use crate::backend::{
    BackendError, GetOptions, HeadOptions, ObjectBackend, ObjectKey, ObjectMetadata, ObjectSource,
    PutMode, PutOptions,
};
use crate::error;
use bytes::Bytes;
use open_compute_core::{ErrorCode, PlatformError, PlatformId, StartupId};
use rand::Rng as _;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use std::fmt::{Debug, Formatter};

const META_SHA256: &str = "sha256";
const AUTHORITY_MARKER: &str = "authority/v1.json";

#[derive(Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AuthorityMarker {
    schema_version: u32,
    platform_id: String,
    backend_kind: open_compute_core::ObjectStorageKind,
    authority_sha256: String,
}

/// Successful preflight. Contains no object keys or secrets.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct PreflightOutcome {
    payload_bytes: usize,
    puts: u8,
    heads: u8,
    gets: u8,
    deletes: u8,
}

impl PreflightOutcome {
    /// Number of bytes written during preflight.
    #[must_use]
    pub const fn payload_bytes(self) -> usize {
        self.payload_bytes
    }

    /// Successful PUT operations completed.
    #[must_use]
    pub const fn puts(self) -> u8 {
        self.puts
    }

    /// Successful HEAD operations completed.
    #[must_use]
    pub const fn heads(self) -> u8 {
        self.heads
    }

    /// Successful GET operations completed.
    #[must_use]
    pub const fn gets(self) -> u8 {
        self.gets
    }

    /// Successful DELETE operations completed.
    #[must_use]
    pub const fn deletes(self) -> u8 {
        self.deletes
    }

    /// Fixture for metrics tests: PUT/HEAD/GET/DELETE/HEAD.
    #[cfg(any(test, feature = "test-support"))]
    #[must_use]
    pub const fn successful_canary() -> Self {
        Self {
            payload_bytes: 32,
            puts: 1,
            heads: 2,
            gets: 1,
            deletes: 1,
        }
    }
}

impl Debug for PreflightOutcome {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PreflightOutcome")
            .field("payload_bytes", &self.payload_bytes)
            .field("puts", &self.puts)
            .field("heads", &self.heads)
            .field("gets", &self.gets)
            .field("deletes", &self.deletes)
            .finish()
    }
}

/// Run PUT/HEAD/GET/DELETE/HEAD preflight under the internal prefix.
pub async fn preflight_object_storage(
    backend: &ObjectBackend,
    platform_id: PlatformId,
    startup_id: StartupId,
) -> Result<PreflightOutcome, PlatformError> {
    ensure_authority_marker(backend, platform_id).await?;
    let mut nonce = [0_u8; 16];
    rand::rng().fill(&mut nonce);
    let key = ObjectKey::new(format!(
        "{}preflight/{platform_id}/{startup_id}/{}",
        backend.prefix(),
        hex::encode(nonce)
    ))
    .map_err(error::from_backend)?;
    let mut payload = [0_u8; 32];
    rand::rng().fill(&mut payload);
    let digest = hex::encode(Sha256::digest(payload));
    let result = run_stages(backend, &key, &payload, &digest).await;
    if result.is_err() {
        let _ = backend.delete(&key).await;
    }
    result
}

/// Verify an already-initialized authority marker without mutating object storage.
pub async fn verify_object_authority(
    backend: &ObjectBackend,
    platform_id: PlatformId,
) -> Result<(), PlatformError> {
    let key = ObjectKey::new(format!("{}{AUTHORITY_MARKER}", backend.prefix()))
        .map_err(error::from_backend)?;
    let expected = AuthorityMarker {
        schema_version: 1,
        platform_id: platform_id.to_string(),
        backend_kind: backend.kind(),
        authority_sha256: hex::encode(backend.authority_sha256()),
    };
    match read_authority_marker(backend, &key).await? {
        Some(found) if found == expected => Ok(()),
        _ => Err(error::from_backend(BackendError::AuthorityMismatch)),
    }
}

async fn ensure_authority_marker(
    backend: &ObjectBackend,
    platform_id: PlatformId,
) -> Result<(), PlatformError> {
    let key = ObjectKey::new(format!("{}{AUTHORITY_MARKER}", backend.prefix()))
        .map_err(error::from_backend)?;
    let expected = AuthorityMarker {
        schema_version: 1,
        platform_id: platform_id.to_string(),
        backend_kind: backend.kind(),
        authority_sha256: hex::encode(backend.authority_sha256()),
    };
    match read_authority_marker(backend, &key).await? {
        Some(found) if found == expected => return Ok(()),
        Some(_) => return Err(error::from_backend(BackendError::AuthorityMismatch)),
        None => {}
    }
    let bytes =
        serde_json::to_vec(&expected).map_err(|_| error::from_backend(BackendError::Corrupt))?;
    match backend
        .put(
            &key,
            ObjectSource::Bytes(Bytes::from(bytes)),
            PutOptions {
                mode: PutMode::CreateOnly,
                metadata: ObjectMetadata {
                    http: crate::ObjectHttpMetadata {
                        content_type: Some("application/json".to_owned()),
                        ..crate::ObjectHttpMetadata::default()
                    },
                    ..ObjectMetadata::default()
                },
                customer_key: None,
            },
        )
        .await
    {
        Ok(_) | Err(BackendError::PreconditionFailed) => {}
        Err(failure) => return Err(error::from_backend(failure)),
    }
    match read_authority_marker(backend, &key).await? {
        Some(found) if found == expected => Ok(()),
        _ => Err(error::from_backend(BackendError::AuthorityMismatch)),
    }
}

async fn read_authority_marker(
    backend: &ObjectBackend,
    key: &ObjectKey,
) -> Result<Option<AuthorityMarker>, PlatformError> {
    let output = match backend.get(key, GetOptions::default()).await {
        Ok(output) => output,
        Err(BackendError::NotFound) => return Ok(None),
        Err(failure) => return Err(error::from_backend(failure)),
    };
    if output.metadata.size == 0 || output.metadata.size > 4096 {
        return Err(error::from_backend(BackendError::Corrupt));
    }
    let bytes = output
        .body
        .collect()
        .await
        .map_err(|_| error::from_backend(BackendError::Unavailable))?
        .into_bytes();
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|_| error::from_backend(BackendError::Corrupt))
}

async fn run_stages(
    backend: &ObjectBackend,
    key: &ObjectKey,
    payload: &[u8],
    digest: &str,
) -> Result<PreflightOutcome, PlatformError> {
    backend
        .put(
            key,
            ObjectSource::Bytes(Bytes::copy_from_slice(payload)),
            PutOptions {
                mode: PutMode::Replace,
                metadata: ObjectMetadata {
                    user: [(META_SHA256.to_owned(), digest.to_owned())]
                        .into_iter()
                        .collect(),
                    ..ObjectMetadata::default()
                },
                customer_key: None,
            },
        )
        .await
        .map_err(error::from_backend)?;
    let head = backend
        .head(key, HeadOptions::default())
        .await
        .map_err(error::from_backend)?;
    if head.size != payload.len() as u64
        || head.user.get(META_SHA256).map(String::as_str) != Some(digest)
    {
        return Err(error::integrity_error());
    }
    let got = backend
        .get(key, GetOptions::default())
        .await
        .map_err(error::from_backend)?;
    let body = got
        .body
        .collect()
        .await
        .map_err(|_| {
            PlatformError::new(
                ErrorCode::ObjectStorageUnavailable,
                "object storage preflight read failed",
            )
        })?
        .into_bytes();
    if body.as_ref() != payload || hex::encode(Sha256::digest(&body)) != digest {
        return Err(error::integrity_error());
    }
    backend.delete(key).await.map_err(error::from_backend)?;
    match backend.head(key, HeadOptions::default()).await {
        Err(BackendError::NotFound) => {}
        Err(failure) => return Err(error::from_backend(failure)),
        Ok(_) => {
            return Err(PlatformError::new(
                ErrorCode::ObjectStorageIntegrityError,
                "object storage delete verification failed",
            ));
        }
    }
    Ok(PreflightOutcome {
        payload_bytes: payload.len(),
        puts: 1,
        heads: 2,
        gets: 1,
        deletes: 1,
    })
}
