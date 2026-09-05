//! Typed object-backend authority for tenant R2 bytes and metadata.

use crate::backend::{
    BackendError, CustomerKey, GetOptions, HeadOptions, ObjectBackend, ObjectKey, ObjectMetadata,
    ObjectRange, open_private_source,
};
use crate::r2_codec::{
    META_CUSTOM, META_HTTP_FIELDS, META_MD5, META_SCHEMA, META_SHA1, META_SHA256, META_SHA384,
    META_SHA512, META_SSEC_MD5, META_STORAGE, META_VERSION, OBJECTS_SUFFIX,
    canonical_custom_metadata, decode_metadata, encode_custom_metadata, integrity_error,
};
use crate::r2_model::{
    R2_MAX_DELETE_KEYS, R2_MAX_LIST_LIMIT, R2BucketIdentity, R2BucketLocator, R2ChecksumAlgorithm,
    R2ComputedChecksums, R2Condition, R2Download, R2GetResult, R2ObjectMetadata, R2PutOptions,
    R2Range, R2SsecKey, R2StorageClass, R2UploadSource, UserObjectKey, checksum_mismatch,
    invalid_options, ssec_invalid,
};
use md5::{Digest as _, Md5};
use open_compute_core::{ErrorCode, PlatformError, ResourceId};
use sha1::Sha1;
use sha2::{Sha256, Sha384, Sha512};
use std::collections::BTreeMap;
use std::path::Path;

const R2_PROVIDER_KEY_MAX_BYTES: usize = 1024;
const R2_PHYSICAL_OBJECT_DIGEST_BYTES: usize = 64;

/// Compile-time typed tenant object store sharing only the selected object backend.
#[derive(Clone, Debug)]
pub struct R2ObjectStore {
    pub(crate) backend: ObjectBackend,
}

impl R2ObjectStore {
    /// Construct the R2-only typed store from the selected authority.
    #[must_use]
    pub const fn new(backend: ObjectBackend) -> Self {
        Self { backend }
    }

    /// Frozen digest of the selected authority descriptor.
    #[must_use]
    pub fn authority_sha256(&self) -> [u8; 32] {
        self.backend.authority_sha256()
    }

    /// Validate a persisted locator against the configured R2 namespace.
    pub fn locator(
        &self,
        resource_id: ResourceId,
        physical_prefix: &str,
    ) -> Result<R2BucketLocator, PlatformError> {
        let expected = format!("{}v1/{resource_id}/", self.backend.r2_prefix());
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
        format!("{}v1/{resource_id}/", self.backend.r2_prefix())
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
        let key = object_key(&locator.identity_marker_key())?;
        let result = self
            .backend
            .put(
                &key,
                crate::ObjectSource::Bytes(bytes::Bytes::from(bytes)),
                crate::PutOptions {
                    mode: crate::PutMode::CreateOnly,
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
            .await;
        if let Err(failure) = result
            && failure != BackendError::PreconditionFailed
        {
            return Err(map_backend(failure));
        }
        if self.read_identity(locator).await?.as_ref() != Some(identity) {
            return Err(prefix_collision());
        }
        Ok(())
    }

    /// Read and validate the immutable bucket identity marker.
    pub async fn read_identity(
        &self,
        locator: &R2BucketLocator,
    ) -> Result<Option<R2BucketIdentity>, PlatformError> {
        let key = object_key(&locator.identity_marker_key())?;
        let output = match self.backend.get(&key, GetOptions::default()).await {
            Ok(output) => output,
            Err(BackendError::NotFound) => return Ok(None),
            Err(failure) => return Err(map_backend(failure)),
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
        serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|_| prefix_collision())
    }

    /// Remove and confirm absence of the immutable identity marker.
    pub async fn delete_identity(&self, locator: &R2BucketLocator) -> Result<(), PlatformError> {
        let key = object_key(&locator.identity_marker_key())?;
        self.backend.delete(&key).await.map_err(map_backend)?;
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
        let physical = object_key(&self.object_key(locator, key))?;
        match self
            .backend
            .head(
                &physical,
                HeadOptions {
                    customer_key: customer_key(ssec),
                },
            )
            .await
        {
            Ok(object) => {
                let metadata = decode_metadata(key.as_str(), &object, None)?;
                check_ssec(&metadata, ssec)?;
                Ok(Some(metadata))
            }
            Err(BackendError::NotFound) => Ok(None),
            Err(BackendError::CustomerKeyInvalid) => Err(ssec_invalid()),
            Err(failure) => Err(map_backend(failure)),
        }
    }

    /// Fetch one object as a backend-neutral stream with stable conditional semantics.
    pub async fn get(
        &self,
        locator: &R2BucketLocator,
        key: &UserObjectKey,
        range: Option<R2Range>,
        condition: Option<&R2Condition>,
        ssec: Option<&R2SsecKey>,
    ) -> Result<R2GetResult, PlatformError> {
        if let Some(range) = range {
            validate_range_shape(range)?;
        }
        let physical = object_key(&self.object_key(locator, key))?;
        for attempt in 0..2 {
            let Some(current) = self.head(locator, key, ssec).await? else {
                return Ok(R2GetResult::Missing);
            };
            if condition
                .is_some_and(|condition| !condition.matches_object(&current.etag, current.uploaded))
            {
                return Ok(R2GetResult::Precondition(current));
            }
            let object_range = range
                .map(|range| resolve_range(range, current.size))
                .transpose()?;
            let result = self
                .backend
                .get(
                    &physical,
                    GetOptions {
                        range: object_range,
                        if_match: condition.map(|_| current.etag.clone()),
                        customer_key: customer_key(ssec),
                    },
                )
                .await;
            let output = match result {
                Ok(output) => output,
                Err(BackendError::NotFound) => return Ok(R2GetResult::Missing),
                Err(BackendError::CustomerKeyInvalid) => return Err(ssec_invalid()),
                Err(BackendError::PreconditionFailed) if condition.is_some() && attempt == 0 => {
                    continue;
                }
                Err(BackendError::PreconditionFailed) => return Err(provider_unavailable()),
                Err(BackendError::InvalidRange) => return Err(invalid_options()),
                Err(failure) => return Err(map_backend(failure)),
            };
            let mut metadata = decode_metadata(key.as_str(), &output.metadata, range)?;
            metadata.range = output.range.map(|returned| R2Range {
                offset: Some(returned.start),
                length: Some(returned.end - returned.start + 1),
                suffix: None,
            });
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
        let physical = keys
            .iter()
            .map(|key| object_key(&self.object_key(locator, key)))
            .collect::<Result<Vec<_>, _>>()?;
        self.backend
            .delete_many(&physical)
            .await
            .map(|_| ())
            .map_err(map_mutation)
    }

    /// Return whether the logical objects prefix currently contains any object.
    pub async fn is_empty(&self, locator: &R2BucketLocator) -> Result<bool, PlatformError> {
        self.list_physical_keys(locator, 1)
            .await
            .map(|keys| keys.is_empty())
    }

    /// Delete at most one full backend page and return whether more work may remain.
    pub async fn delete_first_page(
        &self,
        locator: &R2BucketLocator,
    ) -> Result<bool, PlatformError> {
        let keys = self.list_physical_keys(locator, R2_MAX_LIST_LIMIT).await?;
        if keys.is_empty() {
            return Ok(false);
        }
        self.backend
            .delete_many(&keys)
            .await
            .map_err(map_mutation)?;
        Ok(true)
    }

    async fn list_physical_keys(
        &self,
        locator: &R2BucketLocator,
        limit: u16,
    ) -> Result<Vec<ObjectKey>, PlatformError> {
        let page = self
            .backend
            .list(&locator.object_prefix, limit, None)
            .await
            .map_err(map_backend)?;
        page.objects
            .into_iter()
            .map(|object| {
                object
                    .key
                    .as_str()
                    .starts_with(&locator.object_prefix)
                    .then_some(object.key)
                    .ok_or_else(integrity_error)
            })
            .collect()
    }

    pub(crate) fn object_key(&self, locator: &R2BucketLocator, key: &UserObjectKey) -> String {
        format!(
            "{}{}",
            locator.object_prefix,
            hex::encode(Sha256::digest(key.as_str().as_bytes()))
        )
    }
}

pub(crate) fn validate_upload(
    source: &R2UploadSource,
    options: &R2PutOptions,
) -> Result<(), PlatformError> {
    drop(
        open_private_source(&source.path, source.length).map_err(|failure| {
            if failure == BackendError::Unavailable {
                provider_unavailable()
            } else {
                integrity_error()
            }
        })?,
    );
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
    let mut file = open_private_source(path, expected_length).map_err(|failure| {
        if failure == BackendError::Unavailable {
            provider_unavailable()
        } else {
            integrity_error()
        }
    })?;
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
    custom_metadata: &BTreeMap<String, String>,
    http_metadata: &crate::R2HttpMetadata,
    storage_class: R2StorageClass,
    ssec: Option<&R2SsecKey>,
) -> Result<BTreeMap<String, String>, PlatformError> {
    let mut metadata = BTreeMap::new();
    metadata.insert(META_SCHEMA.to_owned(), "1".to_owned());
    metadata.insert(META_VERSION.to_owned(), version.to_owned());
    let fields = [
        http_metadata.content_type.is_some(),
        http_metadata.content_language.is_some(),
        http_metadata.content_disposition.is_some(),
        http_metadata.content_encoding.is_some(),
        http_metadata.cache_control.is_some(),
        http_metadata.cache_expiry.is_some(),
    ]
    .into_iter()
    .enumerate()
    .fold(0_u8, |mask, (bit, present)| {
        mask | (u8::from(present) << bit)
    });
    metadata.insert(META_HTTP_FIELDS.to_owned(), fields.to_string());
    metadata.insert(
        META_CUSTOM.to_owned(),
        encode_custom_metadata(custom_metadata)?,
    );
    metadata.insert(META_STORAGE.to_owned(), storage_class.as_str().to_owned());
    if let Some(ssec) = ssec {
        metadata.insert(META_SSEC_MD5.to_owned(), ssec.md5_hex());
    }
    Ok(metadata)
}

pub(crate) fn object_user_metadata(
    source: &R2UploadSource,
    options: &R2PutOptions,
) -> Result<BTreeMap<String, String>, PlatformError> {
    create_user_metadata(
        &source.version,
        &options.custom_metadata,
        &options.http_metadata,
        options.storage_class,
        options.ssec.as_ref(),
    )
}

pub(crate) fn apply_checksum_metadata(
    metadata: &mut BTreeMap<String, String>,
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

pub(crate) fn check_ssec(
    metadata: &R2ObjectMetadata,
    ssec: Option<&R2SsecKey>,
) -> Result<(), PlatformError> {
    match (metadata.ssec_key_md5.as_deref(), ssec) {
        (None, _) => Ok(()),
        (Some(expected), Some(ssec)) if expected == ssec.md5_hex() => Ok(()),
        (Some(_), _) => Err(ssec_invalid()),
    }
}

pub(crate) fn customer_key(ssec: Option<&R2SsecKey>) -> Option<CustomerKey> {
    ssec.map(|key| CustomerKey::new(*key.as_bytes()))
}

pub(crate) fn object_key(key: &str) -> Result<ObjectKey, PlatformError> {
    ObjectKey::new(key.to_owned()).map_err(|_| invariant())
}

fn resolve_range(range: R2Range, size: u64) -> Result<ObjectRange, PlatformError> {
    if size == 0 {
        return Err(invalid_options());
    }
    let (start, end) = match (range.offset, range.length, range.suffix) {
        (Some(start), Some(length), None) if length > 0 && start < size => {
            (start, start.saturating_add(length - 1).min(size - 1))
        }
        (Some(start), None, None) if start < size => (start, size - 1),
        (None, Some(length), None) if length > 0 => (0, length.min(size) - 1),
        (None, None, Some(suffix)) if suffix > 0 => (size.saturating_sub(suffix), size - 1),
        _ => return Err(invalid_options()),
    };
    Ok(ObjectRange { start, end })
}

fn validate_range_shape(range: R2Range) -> Result<(), PlatformError> {
    match (range.offset, range.length, range.suffix) {
        (Some(_), Some(length), None) if length > 0 => Ok(()),
        (Some(_), None, None) => Ok(()),
        (None, Some(length), None) if length > 0 => Ok(()),
        (None, None, Some(suffix)) if suffix > 0 => Ok(()),
        _ => Err(invalid_options()),
    }
}

pub(crate) fn map_backend(failure: BackendError) -> PlatformError {
    match failure {
        BackendError::CustomerKeyInvalid => ssec_invalid(),
        BackendError::InvalidRange => invalid_options(),
        BackendError::MultipartInvalid => crate::r2_model::multipart_invalid(),
        BackendError::Capacity => object_too_large(),
        BackendError::Corrupt => integrity_error(),
        BackendError::PreconditionFailed
        | BackendError::Unavailable
        | BackendError::AuthorityMismatch
        | BackendError::InvalidKey
        | BackendError::NotFound => provider_unavailable(),
    }
}

pub(crate) fn map_mutation(failure: BackendError) -> PlatformError {
    match failure {
        BackendError::Unavailable => result_unknown(),
        other => map_backend(other),
    }
}

pub(crate) const fn provider_unavailable() -> PlatformError {
    PlatformError::new(
        ErrorCode::R2ProviderUnavailable,
        "R2 provider operation is unavailable",
    )
}

pub(crate) const fn result_unknown() -> PlatformError {
    PlatformError::new(ErrorCode::R2ResultUnknown, "R2 mutation result is unknown")
}

pub(crate) const fn object_too_large() -> PlatformError {
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

pub(crate) const fn invariant() -> PlatformError {
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
