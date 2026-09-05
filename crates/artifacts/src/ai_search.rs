//! Immutable, content-addressed AI Search source objects.

use crate::backend::open_private_source;
use crate::{
    BackendError, GetOptions, HeadOptions, ObjectBackend, ObjectBody, ObjectKey, ObjectMetadata,
    ObjectSource, PutMode, PutOptions,
};
use open_compute_core::{AccountId, ErrorCode, PlatformError, ResourceId};
use sha2::{Digest as _, Sha256};
use std::io::{Read as _, Seek as _, SeekFrom};
use std::path::Path;

const LAYOUT: &str = "ai-search/v1";
const META_SHA256: &str = "sha256";

/// Exact immutable AI Search source-object identity.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AiSearchObjectRef {
    /// Owning account.
    pub account_id: AccountId,
    /// Owning AI Search instance resource.
    pub instance_resource_id: ResourceId,
    /// Exact object SHA-256 bytes.
    pub sha256: [u8; 32],
    /// Exact object byte length.
    pub size: u64,
}

impl AiSearchObjectRef {
    /// Build an exact source object identity.
    pub fn new(
        account_id: AccountId,
        instance_resource_id: ResourceId,
        sha256: [u8; 32],
        size: u64,
    ) -> Result<Self, PlatformError> {
        if size == 0 || size > 4 * 1024 * 1024 {
            return Err(limit());
        }
        Ok(Self {
            account_id,
            instance_resource_id,
            sha256,
            size,
        })
    }

    /// Canonical object key under the configured system prefix.
    #[must_use]
    pub fn object_key(&self, system_prefix: &str) -> String {
        let digest = hex::encode(self.sha256);
        format!(
            "{system_prefix}{LAYOUT}/{}/{}/objects/sha256/{}/{}",
            self.account_id,
            self.instance_resource_id,
            &digest[..2],
            digest
        )
    }
}

/// Streaming download whose metadata has already matched the exact DB authority.
#[derive(Debug)]
pub struct AiSearchObjectDownload {
    /// Exact content length.
    pub size: u64,
    /// Bounded backend-neutral response stream.
    pub body: ObjectBody,
}

/// Adapter restricted to immutable AI Search source-object keys.
#[derive(Clone, Debug)]
pub struct AiSearchObjectStore {
    backend: ObjectBackend,
}

impl AiSearchObjectStore {
    /// Bind the configured platform object authority.
    #[must_use]
    pub const fn new(backend: ObjectBackend) -> Self {
        Self { backend }
    }

    /// Return the canonical key for one object identity.
    #[must_use]
    pub fn object_key(&self, reference: &AiSearchObjectRef) -> String {
        reference.object_key(self.backend.prefix())
    }

    /// Upload a pre-hashed private staging file with create-only semantics and
    /// verify the resulting remote identity. Existing identical content is an
    /// idempotent success.
    pub async fn put_file(
        &self,
        reference: &AiSearchObjectRef,
        path: &Path,
    ) -> Result<String, PlatformError> {
        let key = self.object_key(reference);
        validate_key(&self.backend, reference, &key)?;
        let mut file = open_private_source(path, reference.size).map_err(|_| invalid())?;
        let mut digest = Sha256::new();
        let mut total = 0_u64;
        let mut buffer = [0_u8; 64 * 1024];
        loop {
            let read = file.read(&mut buffer).map_err(|_| invalid())?;
            if read == 0 {
                break;
            }
            total = total.checked_add(read as u64).ok_or_else(limit)?;
            if total > reference.size {
                return Err(invalid());
            }
            digest.update(&buffer[..read]);
        }
        let computed: [u8; 32] = digest.finalize().into();
        if total != reference.size || computed != reference.sha256 {
            return Err(integrity());
        }
        file.seek(SeekFrom::Start(0)).map_err(|_| invalid())?;
        let sha256 = hex::encode(reference.sha256);
        let physical = ObjectKey::new(key.clone()).map_err(|_| invalid())?;
        let result = self
            .backend
            .put(
                &physical,
                ObjectSource::File {
                    file,
                    length: reference.size,
                },
                PutOptions {
                    mode: PutMode::CreateOnly,
                    metadata: ObjectMetadata {
                        user: [(META_SHA256.to_owned(), sha256)].into_iter().collect(),
                        ..ObjectMetadata::default()
                    },
                    customer_key: None,
                },
            )
            .await;
        if let Err(failure) = result
            && failure != BackendError::PreconditionFailed
        {
            return Err(map_backend(failure));
        }
        self.verify(reference, &key).await?;
        Ok(key)
    }

    /// Verify exact key, length, and digest metadata with a signed HEAD.
    pub async fn verify(
        &self,
        reference: &AiSearchObjectRef,
        key: &str,
    ) -> Result<(), PlatformError> {
        validate_key(&self.backend, reference, key)?;
        let physical = ObjectKey::new(key.to_owned()).map_err(|_| invalid())?;
        let output = self
            .backend
            .head(&physical, HeadOptions::default())
            .await
            .map_err(map_backend)?;
        validate_remote(&output, reference)
    }

    /// Open a download only after exact remote metadata validation.
    pub async fn download(
        &self,
        reference: &AiSearchObjectRef,
        key: &str,
    ) -> Result<AiSearchObjectDownload, PlatformError> {
        validate_key(&self.backend, reference, key)?;
        let physical = ObjectKey::new(key.to_owned()).map_err(|_| invalid())?;
        let output = self
            .backend
            .get(&physical, GetOptions::default())
            .await
            .map_err(map_backend)?;
        validate_remote(&output.metadata, reference)?;
        Ok(AiSearchObjectDownload {
            size: reference.size,
            body: output.body,
        })
    }

    /// Delete one exact, verified object and prove it no longer exists.
    pub async fn delete_exact(
        &self,
        reference: &AiSearchObjectRef,
        key: &str,
    ) -> Result<(), PlatformError> {
        validate_key(&self.backend, reference, key)?;
        let physical = ObjectKey::new(key.to_owned()).map_err(|_| invalid())?;
        let head = self.backend.head(&physical, HeadOptions::default()).await;
        match head {
            Ok(output) => validate_remote(&output, reference)?,
            Err(BackendError::NotFound) => return Ok(()),
            Err(failure) => return Err(map_backend(failure)),
        }
        self.backend.delete(&physical).await.map_err(map_backend)?;
        match self.backend.head(&physical, HeadOptions::default()).await {
            Err(BackendError::NotFound) => Ok(()),
            Err(failure) => Err(map_backend(failure)),
            Ok(_) => Err(integrity()),
        }
    }
}

fn validate_key(
    backend: &ObjectBackend,
    reference: &AiSearchObjectRef,
    key: &str,
) -> Result<(), PlatformError> {
    if key != reference.object_key(backend.prefix()) || key.len() > 1024 {
        return Err(invalid());
    }
    Ok(())
}

fn validate_remote(
    metadata: &ObjectMetadata,
    reference: &AiSearchObjectRef,
) -> Result<(), PlatformError> {
    let expected = hex::encode(reference.sha256);
    if metadata.size != reference.size
        || metadata
            .user
            .get(META_SHA256)
            .is_none_or(|value| value != &expected)
    {
        return Err(integrity());
    }
    Ok(())
}

fn invalid() -> PlatformError {
    PlatformError::new(
        ErrorCode::ResourceInvariantViolation,
        "AI Search object identity is invalid",
    )
}

fn integrity() -> PlatformError {
    PlatformError::new(
        ErrorCode::ArtifactIntegrityError,
        "AI Search object failed integrity verification",
    )
}

fn limit() -> PlatformError {
    PlatformError::new(
        ErrorCode::BindingLimitExceeded,
        "AI Search object exceeds a fixed limit",
    )
}

fn map_backend(error: BackendError) -> PlatformError {
    match error {
        BackendError::Corrupt => integrity(),
        BackendError::Capacity => limit(),
        BackendError::InvalidKey => invalid(),
        _ => PlatformError::new(
            ErrorCode::ArtifactUnavailable,
            "AI Search object authority is unavailable",
        ),
    }
}

#[cfg(test)]
#[path = "ai_search_tests.rs"]
mod tests;
