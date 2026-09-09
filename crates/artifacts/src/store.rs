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

mod backend;

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
    let mut buffer = Box::new([0_u8; 64 * 1024]);
    let mut total = 0_u64;
    loop {
        let count = reader
            .read(&mut buffer[..])
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
