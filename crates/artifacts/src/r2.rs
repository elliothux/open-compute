//! Typed S3-backed authority for tenant R2 object bytes and metadata.

use crate::client::S3ArtifactClient;
use crate::r2_codec::{
    META_CUSTOM, META_MD5, META_SCHEMA, META_SHA1, META_SHA256, META_SHA384, META_SHA512,
    META_SSEC_MD5, META_STORAGE, META_VERSION, OBJECTS_SUFFIX, canonical_custom_metadata,
    decode_metadata, encode_custom_metadata, http_date_millis, integrity_error,
};
use crate::r2_model::{
    R2_MAX_DELETE_KEYS, R2_MAX_LIST_LIMIT, R2BucketIdentity, R2BucketLocator, R2ChecksumAlgorithm,
    R2ComputedChecksums, R2Condition, R2Download, R2GetResult, R2HttpMetadata, R2ObjectMetadata,
    R2PutOptions, R2Range, R2SsecKey, R2StorageClass, R2UploadSource, UserObjectKey,
    checksum_mismatch, invalid_options, ssec_invalid,
};
use aws_sdk_s3::error::SdkError;
use aws_sdk_s3::operation::delete_objects::DeleteObjectsError;
use aws_sdk_s3::operation::get_object::GetObjectError;
use aws_sdk_s3::operation::head_object::HeadObjectError;
use aws_sdk_s3::operation::put_object::PutObjectError;
use aws_sdk_s3::primitives::ByteStream;
use aws_sdk_s3::types::{Delete, ObjectIdentifier};
use aws_smithy_runtime_api::client::orchestrator::HttpResponse;
use base64::Engine as _;
use md5::{Digest as _, Md5};
use open_compute_core::{ErrorCode, PlatformError, ResourceId};
use sha1::Sha1;
use sha2::{Sha256, Sha384, Sha512};
use std::collections::HashMap;
use std::path::Path;

const R2_PROVIDER_KEY_MAX_BYTES: usize = 1024;
const R2_PHYSICAL_OBJECT_DIGEST_BYTES: usize = 64;

/// Compile-time typed tenant object store sharing only the configured S3 client context.
#[derive(Clone, Debug)]
pub struct R2ObjectStore {
    pub(crate) client: S3ArtifactClient,
}

impl R2ObjectStore {
    /// Construct the R2-only typed store from the validated S3 context.
    #[must_use]
    pub fn new(client: S3ArtifactClient) -> Self {
        Self { client }
    }

    /// Frozen digest of endpoint, bucket, region, path style, and both owned prefixes.
    #[must_use]
    pub fn authority_sha256(&self) -> [u8; 32] {
        self.client.authority_sha256()
    }

    /// Validate a persisted locator against the configured R2 namespace.
    pub fn locator(
        &self,
        resource_id: ResourceId,
        physical_prefix: &str,
    ) -> Result<R2BucketLocator, PlatformError> {
        let expected = format!("{}v1/{resource_id}/", self.client.r2_prefix());
        if physical_prefix != expected
            || expected
                .len()
                .saturating_add(OBJECTS_SUFFIX.len())
                .saturating_add(R2_PHYSICAL_OBJECT_DIGEST_BYTES)
                > R2_PROVIDER_KEY_MAX_BYTES
        {
            return Err(PlatformError::new(
                ErrorCode::ResourceInvariantViolation,
                "R2 physical prefix does not match configured authority",
            ));
        }
        Ok(R2BucketLocator {
            resource_id,
            object_prefix: format!("{expected}{OBJECTS_SUFFIX}"),
            physical_prefix: expected,
        })
    }

    /// Canonical physical prefix for a newly allocated resource.
    #[must_use]
    pub fn physical_prefix(&self, resource_id: ResourceId) -> String {
        format!("{}v1/{resource_id}/", self.client.r2_prefix())
    }

    /// Atomically create and verify the immutable bucket identity marker.
    pub async fn ensure_identity(
        &self,
        locator: &R2BucketLocator,
        identity: &R2BucketIdentity,
    ) -> Result<(), PlatformError> {
        if locator.resource_id != identity.resource_id || identity.schema_version != 1 {
            return Err(invariant());
        }
        let bytes = serde_json::to_vec(identity).map_err(|_| invariant())?;
        let key = locator.identity_marker_key();
        let result = self
            .client
            .inner()
            .put_object()
            .bucket(self.client.bucket())
            .key(&key)
            .body(ByteStream::from(bytes.clone()))
            .content_length(i64::try_from(bytes.len()).map_err(|_| invariant())?)
            .content_type("application/json")
            .if_none_match("*")
            .send()
            .await;
        if let Err(error) = result
            && !is_precondition(&error)
        {
            return Err(map_put_failure(&error));
        }
        let found = self.read_identity(locator).await?;
        if found.as_ref() != Some(identity) {
            return Err(PlatformError::new(
                ErrorCode::R2PrefixCollision,
                "R2 physical prefix identity does not match this resource",
            ));
        }
        Ok(())
    }

    /// Read and validate the immutable bucket identity marker.
    pub async fn read_identity(
        &self,
        locator: &R2BucketLocator,
    ) -> Result<Option<R2BucketIdentity>, PlatformError> {
        let key = locator.identity_marker_key();
        let result = self
            .client
            .inner()
            .get_object()
            .bucket(self.client.bucket())
            .key(key)
            .send()
            .await;
        let output = match result {
            Ok(output) => output,
            Err(error) if is_not_found(&error) => return Ok(None),
            Err(error) => return Err(map_get_failure(&error)),
        };
        let bytes = output
            .body
            .collect()
            .await
            .map_err(|_| provider_unavailable())?
            .into_bytes();
        if bytes.len() > 4096 {
            return Err(prefix_collision());
        }
        let identity = serde_json::from_slice(&bytes).map_err(|_| prefix_collision())?;
        Ok(Some(identity))
    }

    /// Remove and confirm absence of the immutable identity marker.
    pub async fn delete_identity(&self, locator: &R2BucketLocator) -> Result<(), PlatformError> {
        let key = locator.identity_marker_key();
        self.client
            .inner()
            .delete_object()
            .bucket(self.client.bucket())
            .key(&key)
            .send()
            .await
            .map_err(|error| map_delete_failure(&error))?;
        if self.read_identity(locator).await?.is_some() {
            return Err(provider_unavailable());
        }
        Ok(())
    }

    /// Read and validate one object's metadata without fetching its body.
    pub async fn head(
        &self,
        locator: &R2BucketLocator,
        key: &UserObjectKey,
        ssec: Option<&R2SsecKey>,
    ) -> Result<Option<R2ObjectMetadata>, PlatformError> {
        let physical = self.object_key(locator, key);
        let mut request = self
            .client
            .inner()
            .head_object()
            .bucket(self.client.bucket())
            .key(physical);
        request = apply_ssec_head(request, ssec);
        let result = request.send().await;
        match result {
            Ok(output) => {
                let metadata = decode_metadata(
                    key.as_str(),
                    output.content_length(),
                    output.e_tag(),
                    output.last_modified(),
                    output.metadata(),
                    R2HttpMetadata {
                        content_type: output.content_type().map(str::to_owned),
                        content_language: output.content_language().map(str::to_owned),
                        content_disposition: output.content_disposition().map(str::to_owned),
                        content_encoding: output.content_encoding().map(str::to_owned),
                        cache_control: output.cache_control().map(str::to_owned),
                        cache_expiry: output.expires_string().and_then(http_date_millis),
                    },
                    None,
                )?;
                if ssec.is_some() {
                    check_ssec(&metadata, ssec)?;
                }
                Ok(Some(metadata))
            }
            Err(error) if is_head_not_found(&error) => Ok(None),
            Err(error) if ssec.is_some() && is_ssec_denied(sdk_status(&error)) => {
                Err(ssec_invalid())
            }
            Err(error) => Err(map_head_failure(&error)),
        }
    }

    /// Fetch one object as a provider-backed stream with stable conditional semantics.
    pub async fn get(
        &self,
        locator: &R2BucketLocator,
        key: &UserObjectKey,
        range: Option<R2Range>,
        condition: Option<&R2Condition>,
        ssec: Option<&R2SsecKey>,
    ) -> Result<R2GetResult, PlatformError> {
        let physical = self.object_key(locator, key);
        for attempt in 0..2 {
            let expected = if let Some(condition) = condition {
                let Some(metadata) = self.head(locator, key, ssec).await? else {
                    return Ok(R2GetResult::Missing);
                };
                if !condition.matches_object(&metadata.etag, metadata.uploaded) {
                    return Ok(R2GetResult::Precondition(metadata));
                }
                Some(metadata.http_etag)
            } else {
                None
            };
            let mut request = self
                .client
                .inner()
                .get_object()
                .bucket(self.client.bucket())
                .key(&physical);
            request = apply_ssec_get(request, ssec);
            if let Some(range) = range {
                request = request.range(range.header()?);
            }
            if let Some(etag) = expected {
                request = request.if_match(etag);
            }
            let output = match request.send().await {
                Ok(output) => output,
                Err(error) if is_not_found(&error) => return Ok(R2GetResult::Missing),
                Err(error) if is_ssec_denied(sdk_status(&error)) => return Err(ssec_invalid()),
                Err(error)
                    if condition.is_some() && is_get_precondition(&error) && attempt == 0 =>
                {
                    continue;
                }
                Err(error) if is_get_precondition(&error) => {
                    return Err(PlatformError::new(
                        ErrorCode::R2ProviderUnavailable,
                        "R2 object changed repeatedly during conditional read",
                    ));
                }
                Err(error) => return Err(map_get_failure(&error)),
            };
            let parsed_range = output.content_range().and_then(parse_content_range);
            let returned_range = parsed_range.map(|(range, _)| range).or(range);
            let full_size = parsed_range
                .and_then(|(_, total)| i64::try_from(total).ok())
                .or(output.content_length());
            let metadata = decode_metadata(
                key.as_str(),
                full_size,
                output.e_tag(),
                output.last_modified(),
                output.metadata(),
                R2HttpMetadata {
                    content_type: output.content_type().map(str::to_owned),
                    content_language: output.content_language().map(str::to_owned),
                    content_disposition: output.content_disposition().map(str::to_owned),
                    content_encoding: output.content_encoding().map(str::to_owned),
                    cache_control: output.cache_control().map(str::to_owned),
                    cache_expiry: output.expires_string().and_then(http_date_millis),
                },
                returned_range,
            )?;
            check_ssec(&metadata, ssec)?;
            return Ok(R2GetResult::Body(R2Download {
                metadata,
                body: output.body,
            }));
        }
        Err(provider_unavailable())
    }

    /// Delete one or more fully validated logical keys.
    pub async fn delete(
        &self,
        locator: &R2BucketLocator,
        keys: &[UserObjectKey],
    ) -> Result<(), PlatformError> {
        if keys.is_empty() || keys.len() > R2_MAX_DELETE_KEYS {
            return Err(invalid_options());
        }
        if keys.len() == 1 {
            return self.delete_one(locator, &keys[0]).await;
        }
        let objects = keys
            .iter()
            .map(|key| self.object_key(locator, key))
            .collect::<Vec<_>>();
        self.delete_provider_keys(&objects).await
    }

    async fn delete_provider_keys(&self, keys: &[String]) -> Result<(), PlatformError> {
        let objects = keys
            .iter()
            .map(|key| {
                ObjectIdentifier::builder()
                    .key(key)
                    .build()
                    .map_err(|_| invalid_options())
            })
            .collect::<Result<Vec<_>, _>>()?;
        let delete = Delete::builder()
            .set_objects(Some(objects))
            .quiet(true)
            .build()
            .map_err(|_| invalid_options())?;
        match self
            .client
            .inner()
            .delete_objects()
            .bucket(self.client.bucket())
            .delete(delete)
            .send()
            .await
        {
            Ok(output) if output.errors().is_empty() => Ok(()),
            Ok(_) => Err(result_unknown()),
            Err(error) if multi_delete_unsupported(&error) => {
                for key in keys {
                    self.delete_physical_one(key).await?;
                }
                Ok(())
            }
            Err(error) => Err(map_multi_delete_failure(&error)),
        }
    }

    /// Return whether the logical objects prefix currently contains any object.
    pub async fn is_empty(&self, locator: &R2BucketLocator) -> Result<bool, PlatformError> {
        self.list_physical_keys(locator, 1)
            .await
            .map(|keys| keys.is_empty())
    }

    /// Delete at most one full provider page and return whether more work may remain.
    pub async fn delete_first_page(
        &self,
        locator: &R2BucketLocator,
    ) -> Result<bool, PlatformError> {
        let keys = self.list_physical_keys(locator, R2_MAX_LIST_LIMIT).await?;
        if keys.is_empty() {
            return Ok(false);
        }
        self.delete_provider_keys(&keys).await?;
        Ok(true)
    }

    async fn list_physical_keys(
        &self,
        locator: &R2BucketLocator,
        limit: u16,
    ) -> Result<Vec<String>, PlatformError> {
        let output = self
            .client
            .inner()
            .list_objects_v2()
            .bucket(self.client.bucket())
            .prefix(&locator.object_prefix)
            .max_keys(i32::from(limit))
            .send()
            .await
            .map_err(|error| crate::error::from_list(&error))
            .map_err(|_| provider_unavailable())?;
        output
            .contents()
            .iter()
            .map(|object| {
                let key = object.key().ok_or_else(integrity_error)?;
                key.starts_with(&locator.object_prefix)
                    .then(|| key.to_owned())
                    .ok_or_else(integrity_error)
            })
            .collect()
    }

    async fn delete_one(
        &self,
        locator: &R2BucketLocator,
        key: &UserObjectKey,
    ) -> Result<(), PlatformError> {
        self.delete_physical_one(&self.object_key(locator, key))
            .await
    }

    async fn delete_physical_one(&self, key: &str) -> Result<(), PlatformError> {
        self.client
            .inner()
            .delete_object()
            .bucket(self.client.bucket())
            .key(key)
            .send()
            .await
            .map_err(|error| map_delete_failure(&error))?;
        Ok(())
    }

    pub(crate) fn object_key(&self, locator: &R2BucketLocator, key: &UserObjectKey) -> String {
        format!(
            "{}{}",
            locator.object_prefix,
            hex::encode(Sha256::digest(key.as_str().as_bytes()))
        )
    }
}

fn validate_upload(source: &R2UploadSource, options: &R2PutOptions) -> Result<(), PlatformError> {
    let metadata = std::fs::metadata(&source.path).map_err(|_| provider_unavailable())?;
    if !metadata.file_type().is_file() || metadata.len() != source.length {
        return Err(integrity_error());
    }
    let version = uuid::Uuid::parse_str(&source.version).map_err(|_| invariant())?;
    if version.get_version_num() != 7 || version.hyphenated().to_string() != source.version {
        return Err(invariant());
    }
    if options
        .checksum
        .as_ref()
        .is_some_and(|expected| !source.checksums.matches(expected))
    {
        return Err(checksum_mismatch());
    }
    canonical_custom_metadata(&options.custom_metadata)?;
    Ok(())
}

/// Compute every pinned checksum for an exact secure staging file.
pub fn hash_file(path: &Path, expected_length: u64) -> Result<R2ComputedChecksums, PlatformError> {
    use std::io::Read as _;
    let mut file = std::fs::File::open(path).map_err(|_| provider_unavailable())?;
    let mut md5 = Md5::new();
    let mut sha1 = Sha1::new();
    let mut sha256 = Sha256::new();
    let mut sha384 = Sha384::new();
    let mut sha512 = Sha512::new();
    let mut total = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer).map_err(|_| provider_unavailable())?;
        if count == 0 {
            break;
        }
        total = total.saturating_add(count as u64);
        if total > expected_length {
            return Err(integrity_error());
        }
        md5.update(&buffer[..count]);
        sha1.update(&buffer[..count]);
        sha256.update(&buffer[..count]);
        sha384.update(&buffer[..count]);
        sha512.update(&buffer[..count]);
    }
    if total != expected_length {
        return Err(integrity_error());
    }
    Ok(R2ComputedChecksums {
        md5: md5.finalize().into(),
        sha1: sha1.finalize().into(),
        sha256: sha256.finalize().into(),
        sha384: sha384.finalize().into(),
        sha512: sha512.finalize().into(),
    })
}

/// Compute MD5 for an exact secure staging file without loading it into memory.
pub fn md5_file(path: &Path, expected_length: u64) -> Result<[u8; 16], PlatformError> {
    hash_file(path, expected_length).map(|checksums| checksums.md5)
}

/// Compute every pinned checksum for an in-memory buffer.
#[must_use]
pub fn hash_bytes(bytes: &[u8]) -> R2ComputedChecksums {
    R2ComputedChecksums {
        md5: Md5::digest(bytes).into(),
        sha1: Sha1::digest(bytes).into(),
        sha256: Sha256::digest(bytes).into(),
        sha384: Sha384::digest(bytes).into(),
        sha512: Sha512::digest(bytes).into(),
    }
}

pub(crate) fn create_user_metadata(
    version: &str,
    custom_metadata: &std::collections::BTreeMap<String, String>,
    storage_class: R2StorageClass,
    ssec: Option<&R2SsecKey>,
) -> Result<HashMap<String, String>, PlatformError> {
    let mut metadata = HashMap::new();
    metadata.insert(META_SCHEMA.to_owned(), "1".to_owned());
    metadata.insert(META_VERSION.to_owned(), version.to_owned());
    metadata.insert(
        META_CUSTOM.to_owned(),
        encode_custom_metadata(custom_metadata)?,
    );
    metadata.insert(META_STORAGE.to_owned(), storage_class.as_str().to_owned());
    if let Some(ssec) = ssec {
        metadata.insert(META_SSEC_MD5.to_owned(), ssec.md5_base64());
    }
    Ok(metadata)
}

fn object_user_metadata(
    source: &R2UploadSource,
    options: &R2PutOptions,
) -> Result<HashMap<String, String>, PlatformError> {
    create_user_metadata(
        &source.version,
        &options.custom_metadata,
        options.storage_class,
        options.ssec.as_ref(),
    )
}

pub(crate) fn apply_checksum_metadata(
    metadata: &mut HashMap<String, String>,
    checksums: &R2ComputedChecksums,
    requested: Option<&R2ChecksumAlgorithm>,
) {
    let exposed = checksums.exposed(requested);
    for (name, value) in [
        (META_MD5, exposed.md5),
        (META_SHA1, exposed.sha1),
        (META_SHA256, exposed.sha256),
        (META_SHA384, exposed.sha384),
        (META_SHA512, exposed.sha512),
    ] {
        if let Some(value) = value {
            metadata.insert(name.to_owned(), value);
        }
    }
}

fn apply_ssec_put(
    request: aws_sdk_s3::operation::put_object::builders::PutObjectFluentBuilder,
    ssec: Option<&R2SsecKey>,
) -> aws_sdk_s3::operation::put_object::builders::PutObjectFluentBuilder {
    match ssec {
        Some(ssec) => request
            .sse_customer_algorithm("AES256")
            .sse_customer_key(ssec.base64())
            .sse_customer_key_md5(ssec.md5_base64()),
        None => request,
    }
}

fn apply_ssec_get(
    request: aws_sdk_s3::operation::get_object::builders::GetObjectFluentBuilder,
    ssec: Option<&R2SsecKey>,
) -> aws_sdk_s3::operation::get_object::builders::GetObjectFluentBuilder {
    match ssec {
        Some(ssec) => request
            .sse_customer_algorithm("AES256")
            .sse_customer_key(ssec.base64())
            .sse_customer_key_md5(ssec.md5_base64()),
        None => request,
    }
}

fn apply_ssec_head(
    request: aws_sdk_s3::operation::head_object::builders::HeadObjectFluentBuilder,
    ssec: Option<&R2SsecKey>,
) -> aws_sdk_s3::operation::head_object::builders::HeadObjectFluentBuilder {
    match ssec {
        Some(ssec) => request
            .sse_customer_algorithm("AES256")
            .sse_customer_key(ssec.base64())
            .sse_customer_key_md5(ssec.md5_base64()),
        None => request,
    }
}

fn apply_put_checksum(
    request: aws_sdk_s3::operation::put_object::builders::PutObjectFluentBuilder,
    checksum: Option<&R2ChecksumAlgorithm>,
) -> aws_sdk_s3::operation::put_object::builders::PutObjectFluentBuilder {
    match checksum {
        Some(R2ChecksumAlgorithm::Sha1(value)) => {
            request.checksum_sha1(base64::engine::general_purpose::STANDARD.encode(value))
        }
        Some(R2ChecksumAlgorithm::Sha256(value)) => {
            request.checksum_sha256(base64::engine::general_purpose::STANDARD.encode(value))
        }
        _ => request,
    }
}

fn check_ssec(metadata: &R2ObjectMetadata, ssec: Option<&R2SsecKey>) -> Result<(), PlatformError> {
    match (metadata.ssec_key_md5.as_deref(), ssec) {
        (None, _) => Ok(()),
        (Some(expected), Some(ssec)) if expected == ssec.md5_base64() => Ok(()),
        (Some(_), _) => Err(ssec_invalid()),
    }
}

fn is_ssec_denied(status: Option<u16>) -> bool {
    matches!(status, Some(400 | 403))
}

fn parse_content_range(value: &str) -> Option<(R2Range, u64)> {
    let value = value.strip_prefix("bytes ")?;
    let (bounds, total) = value.split_once('/')?;
    let (start, end) = bounds.split_once('-')?;
    let start = start.parse::<u64>().ok()?;
    let end = end.parse::<u64>().ok()?;
    Some((
        R2Range {
            offset: Some(start),
            length: end.checked_sub(start)?.checked_add(1),
            suffix: None,
        },
        total.parse().ok()?,
    ))
}

pub(crate) fn sdk_status<E>(error: &SdkError<E, HttpResponse>) -> Option<u16> {
    match error {
        SdkError::ServiceError(service) => Some(service.raw().status().as_u16()),
        SdkError::ResponseError(response) => Some(response.raw().status().as_u16()),
        _ => None,
    }
}

fn is_precondition(error: &SdkError<PutObjectError, HttpResponse>) -> bool {
    matches!(sdk_status(error), Some(409 | 412))
}

fn is_get_precondition(error: &SdkError<GetObjectError, HttpResponse>) -> bool {
    matches!(sdk_status(error), Some(304 | 412))
}

fn is_not_found(error: &SdkError<GetObjectError, HttpResponse>) -> bool {
    sdk_status(error) == Some(404)
}

fn is_head_not_found(error: &SdkError<HeadObjectError, HttpResponse>) -> bool {
    sdk_status(error) == Some(404)
}

fn multi_delete_unsupported(error: &SdkError<DeleteObjectsError, HttpResponse>) -> bool {
    matches!(sdk_status(error), Some(405 | 501))
}

fn map_put_failure(error: &SdkError<PutObjectError, HttpResponse>) -> PlatformError {
    if sdk_status(error).is_none() || sdk_status(error).is_some_and(|status| status >= 500) {
        result_unknown()
    } else {
        provider_unavailable()
    }
}

fn map_delete_failure<E>(error: &SdkError<E, HttpResponse>) -> PlatformError {
    if sdk_status(error).is_none() || sdk_status(error).is_some_and(|status| status >= 500) {
        result_unknown()
    } else {
        provider_unavailable()
    }
}

fn map_multi_delete_failure(error: &SdkError<DeleteObjectsError, HttpResponse>) -> PlatformError {
    map_delete_failure(error)
}

fn map_get_failure(error: &SdkError<GetObjectError, HttpResponse>) -> PlatformError {
    if sdk_status(error) == Some(416) {
        invalid_options()
    } else {
        provider_unavailable()
    }
}

fn map_head_failure(_error: &SdkError<HeadObjectError, HttpResponse>) -> PlatformError {
    provider_unavailable()
}

pub(crate) fn provider_unavailable() -> PlatformError {
    PlatformError::new(
        ErrorCode::R2ProviderUnavailable,
        "R2 provider operation is unavailable",
    )
}

pub(crate) fn result_unknown() -> PlatformError {
    PlatformError::new(ErrorCode::R2ResultUnknown, "R2 mutation result is unknown")
}

pub(crate) fn object_too_large() -> PlatformError {
    PlatformError::new(
        ErrorCode::R2ObjectTooLarge,
        "R2 object exceeds the frozen single-part limit",
    )
}

fn prefix_collision() -> PlatformError {
    PlatformError::new(
        ErrorCode::R2PrefixCollision,
        "R2 physical prefix identity does not match this resource",
    )
}

pub(crate) fn invariant() -> PlatformError {
    PlatformError::new(
        ErrorCode::ResourceInvariantViolation,
        "R2 typed store invariant failed",
    )
}

#[path = "r2_put.rs"]
mod put;

#[cfg(test)]
#[path = "r2_tests.rs"]
mod tests;
