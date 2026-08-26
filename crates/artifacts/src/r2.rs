//! Typed S3-backed authority for tenant R2 object bytes and metadata.

use crate::client::S3ArtifactClient;
use crate::r2_model::{
    R2_MAX_CUSTOM_METADATA_JSON_BYTES, R2_MAX_DELETE_KEYS, R2_MAX_LIST_LIMIT,
    R2_PROVIDER_KEY_MAX_BYTES, R2BucketIdentity, R2BucketLocator, R2Condition, R2Download,
    R2GetResult, R2HttpMetadata, R2ListPage, R2ListedObject, R2ObjectMetadata, R2PutOptions,
    R2Range, R2UploadSource, UserObjectKey,
};
use aws_sdk_s3::error::SdkError;
use aws_sdk_s3::operation::delete_objects::DeleteObjectsError;
use aws_sdk_s3::operation::get_object::GetObjectError;
use aws_sdk_s3::operation::head_object::HeadObjectError;
use aws_sdk_s3::operation::put_object::PutObjectError;
use aws_sdk_s3::primitives::{ByteStream, DateTime};
use aws_sdk_s3::types::{Delete, ObjectIdentifier};
use aws_smithy_runtime_api::client::orchestrator::HttpResponse;
use aws_smithy_types::date_time::Format as DateTimeFormat;
use base64::Engine as _;
use md5::{Digest as _, Md5};
use open_compute_core::{ErrorCode, PlatformError, ResourceId};
use std::collections::{BTreeMap, HashMap};
use std::path::Path;

const META_SCHEMA: &str = "oc-r2-schema";
const META_VERSION: &str = "oc-r2-version";
const META_CUSTOM: &str = "oc-r2-custom";
const META_MD5: &str = "oc-r2-md5";
const OBJECTS_SUFFIX: &str = "objects/";

/// Compile-time typed tenant object store sharing only the configured S3 client context.
#[derive(Clone, Debug)]
pub struct R2ObjectStore {
    client: S3ArtifactClient,
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
        if physical_prefix != expected || expected.len() >= R2_PROVIDER_KEY_MAX_BYTES {
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
    ) -> Result<Option<R2ObjectMetadata>, PlatformError> {
        let physical = self.object_key(locator, key);
        let result = self
            .client
            .inner()
            .head_object()
            .bucket(self.client.bucket())
            .key(physical)
            .send()
            .await;
        match result {
            Ok(output) => decode_metadata(
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
            )
            .map(Some),
            Err(error) if is_head_not_found(&error) => Ok(None),
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
    ) -> Result<R2GetResult, PlatformError> {
        let physical = self.object_key(locator, key);
        for attempt in 0..2 {
            let expected = if let Some(condition) = condition {
                let Some(metadata) = self.head(locator, key).await? else {
                    return Ok(R2GetResult::Missing);
                };
                if !condition_matches(condition, &metadata) {
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
            if let Some(range) = range {
                request = request.range(range.header()?);
            }
            if let Some(etag) = expected {
                request = request.if_match(etag);
            }
            let output = match request.send().await {
                Ok(output) => output,
                Err(error) if is_not_found(&error) => return Ok(R2GetResult::Missing),
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
            return Ok(R2GetResult::Body(R2Download {
                metadata,
                body: output.body,
            }));
        }
        Err(provider_unavailable())
    }

    /// Upload one already-staged, replayable single-part object and verify it by HEAD.
    pub async fn put_file(
        &self,
        locator: &R2BucketLocator,
        key: &UserObjectKey,
        source: &R2UploadSource,
        options: &R2PutOptions,
    ) -> Result<Option<R2ObjectMetadata>, PlatformError> {
        validate_upload(source, options)?;
        let body = ByteStream::read_from()
            .path(&source.path)
            .length(aws_smithy_types::byte_stream::Length::Exact(source.length))
            .buffer_size(64 * 1024)
            .build()
            .await
            .map_err(|_| provider_unavailable())?;
        let custom = canonical_custom_metadata(&options.custom_metadata)?;
        let mut metadata = HashMap::new();
        metadata.insert(META_SCHEMA.to_owned(), "1".to_owned());
        metadata.insert(META_VERSION.to_owned(), source.version.clone());
        metadata.insert(
            META_CUSTOM.to_owned(),
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(custom),
        );
        metadata.insert(META_MD5.to_owned(), hex::encode(source.md5));
        let mut request = self
            .client
            .inner()
            .put_object()
            .bucket(self.client.bucket())
            .key(self.object_key(locator, key))
            .body(body)
            .content_length(i64::try_from(source.length).map_err(|_| object_too_large())?)
            .set_metadata(Some(metadata))
            .set_content_type(options.http_metadata.content_type.clone())
            .set_content_language(options.http_metadata.content_language.clone())
            .set_content_disposition(options.http_metadata.content_disposition.clone())
            .set_content_encoding(options.http_metadata.content_encoding.clone())
            .set_cache_control(options.http_metadata.cache_control.clone())
            .set_expires(options.http_metadata.cache_expiry.map(millis_datetime))
            .content_md5(base64::engine::general_purpose::STANDARD.encode(source.md5));
        if let Some(condition) = &options.only_if {
            if condition.uploaded_before.is_some() || condition.uploaded_after.is_some() {
                return Err(PlatformError::new(
                    ErrorCode::R2UnsupportedCondition,
                    "R2 conditional PUT supports only atomic ETag conditions",
                ));
            }
            match (
                condition.etag_matches.as_slice(),
                condition.etag_does_not_match.as_slice(),
            ) {
                ([etag], []) => request = request.if_match(quote_etag(etag)?),
                ([], [etag]) => request = request.if_none_match(quote_etag(etag)?),
                ([], []) => {}
                _ => return Err(invalid_options()),
            }
        }
        match request.send().await {
            Ok(_) => {}
            Err(error) if is_precondition(&error) => return Ok(None),
            Err(error) => return Err(map_put_failure(&error)),
        }
        let metadata = self.head(locator, key).await?.ok_or_else(integrity_error)?;
        if metadata.size != source.length
            || metadata.version != source.version
            || metadata.md5 != hex::encode(source.md5)
        {
            return Err(integrity_error());
        }
        Ok(Some(metadata))
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
            .map(|key| {
                ObjectIdentifier::builder()
                    .key(self.object_key(locator, key))
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
                    self.delete_one(locator, key).await?;
                }
                Ok(())
            }
            Err(error) => Err(map_multi_delete_failure(&error)),
        }
    }

    /// List one exact logical prefix without exposing the provider continuation token.
    pub async fn list(
        &self,
        locator: &R2BucketLocator,
        prefix: &str,
        delimiter: Option<&str>,
        limit: u16,
        provider_token: Option<&str>,
    ) -> Result<R2ListPage, PlatformError> {
        let logical_prefix = UserObjectKey::parse(prefix, locator)?;
        if limit == 0 || limit > R2_MAX_LIST_LIMIT {
            return Err(invalid_options());
        }
        if delimiter.is_some_and(str::is_empty) {
            return Err(invalid_options());
        }
        let physical_prefix = self.object_key(locator, &logical_prefix);
        let mut request = self
            .client
            .inner()
            .list_objects_v2()
            .bucket(self.client.bucket())
            .prefix(&physical_prefix)
            .max_keys(i32::from(limit));
        if let Some(delimiter) = delimiter {
            request = request.delimiter(delimiter);
        }
        if let Some(token) = provider_token {
            request = request.continuation_token(token);
        }
        let output = request
            .send()
            .await
            .map_err(|error| crate::error::from_list(&error))
            .map_err(|_| provider_unavailable())?;
        let mut objects = Vec::with_capacity(output.contents().len());
        for object in output.contents() {
            let physical = object.key().ok_or_else(integrity_error)?;
            let key = physical
                .strip_prefix(&locator.object_prefix)
                .ok_or_else(integrity_error)?;
            UserObjectKey::parse(key, locator)?;
            objects.push(R2ListedObject {
                key: key.to_owned(),
                size: u64::try_from(object.size().unwrap_or(-1)).map_err(|_| integrity_error())?,
                etag: unquote_etag(object.e_tag().ok_or_else(integrity_error)?)?,
                uploaded: object.last_modified().map_or(0, datetime_millis),
            });
        }
        let mut delimited_prefixes = Vec::with_capacity(output.common_prefixes().len());
        for common in output.common_prefixes() {
            let physical = common.prefix().ok_or_else(integrity_error)?;
            let logical = physical
                .strip_prefix(&locator.object_prefix)
                .ok_or_else(integrity_error)?;
            delimited_prefixes.push(logical.to_owned());
        }
        Ok(R2ListPage {
            objects,
            delimited_prefixes,
            provider_token: output.next_continuation_token().map(str::to_owned),
        })
    }

    /// Return whether the logical objects prefix currently contains any object.
    pub async fn is_empty(&self, locator: &R2BucketLocator) -> Result<bool, PlatformError> {
        self.list(locator, "", None, 1, None)
            .await
            .map(|page| page.objects.is_empty())
    }

    /// Delete at most one full provider page and return whether more work may remain.
    pub async fn delete_first_page(
        &self,
        locator: &R2BucketLocator,
    ) -> Result<bool, PlatformError> {
        let page = self
            .list(locator, "", None, R2_MAX_LIST_LIMIT, None)
            .await?;
        if page.objects.is_empty() {
            return Ok(false);
        }
        let keys = page
            .objects
            .iter()
            .map(|object| UserObjectKey::parse(&object.key, locator))
            .collect::<Result<Vec<_>, _>>()?;
        self.delete(locator, &keys).await?;
        Ok(true)
    }

    async fn delete_one(
        &self,
        locator: &R2BucketLocator,
        key: &UserObjectKey,
    ) -> Result<(), PlatformError> {
        self.client
            .inner()
            .delete_object()
            .bucket(self.client.bucket())
            .key(self.object_key(locator, key))
            .send()
            .await
            .map_err(|error| map_delete_failure(&error))?;
        Ok(())
    }

    fn object_key(&self, locator: &R2BucketLocator, key: &UserObjectKey) -> String {
        format!("{}{}", locator.object_prefix, key.as_str())
    }
}

fn canonical_custom_metadata(
    metadata: &BTreeMap<String, String>,
) -> Result<Vec<u8>, PlatformError> {
    let bytes = serde_json::to_vec(metadata).map_err(|_| invalid_options())?;
    if bytes.len() > R2_MAX_CUSTOM_METADATA_JSON_BYTES {
        return Err(PlatformError::new(
            ErrorCode::R2MetadataTooLarge,
            "R2 custom metadata exceeds the canonical JSON budget",
        ));
    }
    Ok(bytes)
}

fn decode_metadata(
    key: &str,
    content_length: Option<i64>,
    etag: Option<&str>,
    modified: Option<&DateTime>,
    metadata: Option<&HashMap<String, String>>,
    http_metadata: R2HttpMetadata,
    range: Option<R2Range>,
) -> Result<R2ObjectMetadata, PlatformError> {
    let metadata = metadata.ok_or_else(integrity_error)?;
    if metadata.get(META_SCHEMA).map(String::as_str) != Some("1") {
        return Err(integrity_error());
    }
    let version = metadata.get(META_VERSION).ok_or_else(integrity_error)?;
    let parsed = uuid::Uuid::parse_str(version).map_err(|_| integrity_error())?;
    if parsed.get_version_num() != 7 || parsed.hyphenated().to_string() != *version {
        return Err(integrity_error());
    }
    let custom = metadata.get(META_CUSTOM).ok_or_else(integrity_error)?;
    let custom_bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(custom)
        .map_err(|_| integrity_error())?;
    if custom_bytes.len() > R2_MAX_CUSTOM_METADATA_JSON_BYTES {
        return Err(integrity_error());
    }
    let custom_metadata: BTreeMap<String, String> =
        serde_json::from_slice(&custom_bytes).map_err(|_| integrity_error())?;
    if canonical_custom_metadata(&custom_metadata).map_err(|_| integrity_error())? != custom_bytes {
        return Err(integrity_error());
    }
    let md5 = metadata.get(META_MD5).ok_or_else(integrity_error)?;
    if md5.len() != 32
        || hex::decode(md5).is_err()
        || md5.bytes().any(|byte| byte.is_ascii_uppercase())
    {
        return Err(integrity_error());
    }
    let etag = unquote_etag(etag.ok_or_else(integrity_error)?)?;
    let http_etag = quote_etag(&etag)?;
    let size = u64::try_from(content_length.ok_or_else(integrity_error)?)
        .map_err(|_| integrity_error())?;
    Ok(R2ObjectMetadata {
        key: key.to_owned(),
        version: version.to_owned(),
        size,
        etag,
        http_etag,
        uploaded: modified.map_or(0, datetime_millis),
        http_metadata,
        custom_metadata,
        range,
        md5: md5.to_owned(),
        storage_class: "Standard".to_owned(),
    })
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
        .expected_md5
        .is_some_and(|expected| expected != source.md5)
    {
        return Err(PlatformError::new(
            ErrorCode::R2PreconditionFailed,
            "R2 caller MD5 does not match staged object bytes",
        ));
    }
    canonical_custom_metadata(&options.custom_metadata)?;
    Ok(())
}

/// Compute MD5 for an exact secure staging file without loading it into memory.
pub fn md5_file(path: &Path, expected_length: u64) -> Result<[u8; 16], PlatformError> {
    use std::io::Read as _;
    let mut file = std::fs::File::open(path).map_err(|_| provider_unavailable())?;
    let mut hasher = Md5::new();
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
        hasher.update(&buffer[..count]);
    }
    if total != expected_length {
        return Err(integrity_error());
    }
    Ok(hasher.finalize().into())
}

fn condition_matches(condition: &R2Condition, metadata: &R2ObjectMetadata) -> bool {
    (condition.etag_matches.is_empty()
        || condition
            .etag_matches
            .iter()
            .any(|etag| etag == &metadata.etag || etag == &metadata.http_etag))
        && condition
            .etag_does_not_match
            .iter()
            .all(|etag| etag != &metadata.etag && etag != &metadata.http_etag)
        && condition
            .uploaded_before
            .is_none_or(|time| metadata.uploaded <= time)
        && condition
            .uploaded_after
            .is_none_or(|time| metadata.uploaded > time)
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

fn unquote_etag(value: &str) -> Result<String, PlatformError> {
    let value = value
        .strip_prefix('"')
        .and_then(|v| v.strip_suffix('"'))
        .unwrap_or(value);
    if value.is_empty()
        || value
            .bytes()
            .any(|byte| byte.is_ascii_control() || byte == b'"')
    {
        return Err(integrity_error());
    }
    Ok(value.to_owned())
}

fn quote_etag(value: &str) -> Result<String, PlatformError> {
    let value = unquote_etag(value)?;
    Ok(format!("\"{value}\""))
}

fn datetime_millis(value: &DateTime) -> i64 {
    (*value).to_millis().unwrap_or(0)
}

fn http_date_millis(value: &str) -> Option<i64> {
    DateTime::from_str(value, DateTimeFormat::HttpDate)
        .ok()
        .and_then(|date| date.to_millis().ok())
}

fn millis_datetime(value: i64) -> DateTime {
    let seconds = value.div_euclid(1000);
    let nanos = u32::try_from(value.rem_euclid(1000)).unwrap_or(0) * 1_000_000;
    DateTime::from_secs_and_nanos(seconds, nanos)
}

fn sdk_status<E>(error: &SdkError<E, HttpResponse>) -> Option<u16> {
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

fn provider_unavailable() -> PlatformError {
    PlatformError::new(
        ErrorCode::R2ProviderUnavailable,
        "R2 provider operation is unavailable",
    )
}

fn result_unknown() -> PlatformError {
    PlatformError::new(ErrorCode::R2ResultUnknown, "R2 mutation result is unknown")
}

fn invalid_options() -> PlatformError {
    PlatformError::new(ErrorCode::R2InvalidOptions, "R2 options are invalid")
}

fn object_too_large() -> PlatformError {
    PlatformError::new(
        ErrorCode::R2ObjectTooLarge,
        "R2 object exceeds the frozen single-part limit",
    )
}

fn integrity_error() -> PlatformError {
    PlatformError::new(
        ErrorCode::R2ObjectMetadataInvalid,
        "R2 object metadata failed integrity validation",
    )
}

fn prefix_collision() -> PlatformError {
    PlatformError::new(
        ErrorCode::R2PrefixCollision,
        "R2 physical prefix identity does not match this resource",
    )
}

fn invariant() -> PlatformError {
    PlatformError::new(
        ErrorCode::ResourceInvariantViolation,
        "R2 typed store invariant failed",
    )
}

#[cfg(test)]
#[path = "r2_tests.rs"]
mod tests;
