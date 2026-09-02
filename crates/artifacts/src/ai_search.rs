//! Immutable, content-addressed AI Search source objects in system S3 authority.

use crate::{S3ArtifactClient, error};
use aws_sdk_s3::error::SdkError;
use aws_sdk_s3::operation::head_object::HeadObjectError;
use aws_sdk_s3::primitives::ByteStream;
use aws_smithy_types::byte_stream::Length;
use open_compute_core::{AccountId, ErrorCode, PlatformError, ResourceId};
use sha2::{Digest as _, Sha256};
use std::io::{Read as _, Seek as _, SeekFrom};
use std::os::unix::fs::OpenOptionsExt as _;
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

    /// Canonical S3 key under the configured system prefix.
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
    /// Bounded S3 response stream.
    pub body: ByteStream,
}

/// S3 adapter restricted to immutable AI Search source-object keys.
#[derive(Clone, Debug)]
pub struct AiSearchObjectStore {
    client: S3ArtifactClient,
}

impl AiSearchObjectStore {
    /// Bind the configured platform S3 authority.
    #[must_use]
    pub const fn new(client: S3ArtifactClient) -> Self {
        Self { client }
    }

    /// Return the canonical key for one object identity.
    #[must_use]
    pub fn object_key(&self, reference: &AiSearchObjectRef) -> String {
        reference.object_key(self.client.prefix())
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
        validate_key(&self.client, reference, &key)?;
        let mut file = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
            .open(path)
            .map_err(|_| invalid())?;
        let metadata = file.metadata().map_err(|_| invalid())?;
        if !metadata.file_type().is_file() || metadata.len() != reference.size {
            return Err(invalid());
        }
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
        let body = ByteStream::read_from()
            .file(tokio::fs::File::from_std(file))
            .length(Length::Exact(reference.size))
            .buffer_size(64 * 1024)
            .build()
            .await
            .map_err(|_| invalid())?;
        let sha256 = hex::encode(reference.sha256);
        let result = self
            .client
            .inner()
            .put_object()
            .bucket(self.client.bucket())
            .key(&key)
            .body(body)
            .content_length(i64::try_from(reference.size).map_err(|_| limit())?)
            .metadata(META_SHA256, &sha256)
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
        self.verify(reference, &key).await?;
        Ok(key)
    }

    /// Verify exact key, length, and digest metadata with a signed HEAD.
    pub async fn verify(
        &self,
        reference: &AiSearchObjectRef,
        key: &str,
    ) -> Result<(), PlatformError> {
        validate_key(&self.client, reference, key)?;
        let output = self
            .client
            .inner()
            .head_object()
            .bucket(self.client.bucket())
            .key(key)
            .send()
            .await
            .map_err(|failure| error::from_head(&failure))?;
        validate_remote(output.content_length(), output.metadata(), reference)
    }

    /// Open a download only after exact remote metadata validation.
    pub async fn download(
        &self,
        reference: &AiSearchObjectRef,
        key: &str,
    ) -> Result<AiSearchObjectDownload, PlatformError> {
        validate_key(&self.client, reference, key)?;
        let output = self
            .client
            .inner()
            .get_object()
            .bucket(self.client.bucket())
            .key(key)
            .send()
            .await
            .map_err(|failure| error::from_get(&failure))?;
        validate_remote(output.content_length(), output.metadata(), reference)?;
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
        validate_key(&self.client, reference, key)?;
        let head = self
            .client
            .inner()
            .head_object()
            .bucket(self.client.bucket())
            .key(key)
            .send()
            .await;
        match head {
            Ok(output) => {
                validate_remote(output.content_length(), output.metadata(), reference)?;
            }
            Err(failure) if head_not_found(&failure) => return Ok(()),
            Err(failure) => return Err(error::from_head(&failure)),
        }
        self.client
            .inner()
            .delete_object()
            .bucket(self.client.bucket())
            .key(key)
            .send()
            .await
            .map_err(|failure| error::from_delete(&failure))?;
        match self
            .client
            .inner()
            .head_object()
            .bucket(self.client.bucket())
            .key(key)
            .send()
            .await
        {
            Err(failure) if head_not_found(&failure) => Ok(()),
            Err(failure) => Err(error::from_head(&failure)),
            Ok(_) => Err(integrity()),
        }
    }
}

fn validate_key(
    client: &S3ArtifactClient,
    reference: &AiSearchObjectRef,
    key: &str,
) -> Result<(), PlatformError> {
    if key != reference.object_key(client.prefix()) || key.len() > 1024 {
        return Err(invalid());
    }
    Ok(())
}

fn validate_remote(
    length: Option<i64>,
    metadata: Option<&std::collections::HashMap<String, String>>,
    reference: &AiSearchObjectRef,
) -> Result<(), PlatformError> {
    let expected = hex::encode(reference.sha256);
    if u64::try_from(length.unwrap_or(-1)).ok() != Some(reference.size)
        || metadata
            .and_then(|values| values.get(META_SHA256))
            .is_none_or(|value| value != &expected)
    {
        return Err(integrity());
    }
    Ok(())
}

fn head_not_found(
    failure: &SdkError<HeadObjectError, aws_smithy_runtime_api::client::orchestrator::HttpResponse>,
) -> bool {
    matches!(
        failure,
        SdkError::ServiceError(service) if service.raw().status().as_u16() == 404
    )
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

#[cfg(test)]
#[path = "ai_search_tests.rs"]
mod tests;
