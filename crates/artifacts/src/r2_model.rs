//! Boundary types shared by the typed R2 object store and its callers.

use aws_sdk_s3::primitives::ByteStream;
use open_compute_core::{ErrorCode, PlatformError, PlatformId, ResourceId};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

/// Maximum key bytes accepted by S3 and Cloudflare-compatible R2 APIs.
pub const R2_PROVIDER_KEY_MAX_BYTES: usize = 1024;
/// Maximum keys accepted by one delete call.
pub const R2_MAX_DELETE_KEYS: usize = 1000;
/// Default and maximum list page size.
pub const R2_MAX_LIST_LIMIT: u16 = 1000;
/// Maximum canonical custom metadata bytes before base64 wrapping.
pub const R2_MAX_CUSTOM_METADATA_JSON_BYTES: usize = 4 * 1024;

/// Immutable logical-bucket marker stored below its physical prefix.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct R2BucketIdentity {
    /// Marker schema version.
    pub schema_version: u32,
    /// Owning platform authority.
    pub platform_id: PlatformId,
    /// Immutable logical resource identity.
    pub resource_id: ResourceId,
    /// Resource creation timestamp.
    pub created_at_ms: i64,
}

/// Validated physical locator. Raw keys remain private to this crate.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct R2BucketLocator {
    pub(crate) resource_id: ResourceId,
    pub(crate) physical_prefix: String,
    pub(crate) object_prefix: String,
}

impl R2BucketLocator {
    /// Immutable logical resource identity used by cursor and authorization layers.
    #[must_use]
    pub const fn resource_id(&self) -> ResourceId {
        self.resource_id
    }

    /// Maximum user-key bytes after accounting for the frozen physical prefix.
    #[must_use]
    pub fn max_user_key_bytes(&self) -> usize {
        R2_PROVIDER_KEY_MAX_BYTES.saturating_sub(self.object_prefix.len())
    }
}

/// User object key validated without trimming, decoding, or normalization.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct UserObjectKey(String);

impl UserObjectKey {
    /// Validate a tenant key against one bucket's exact provider-key budget.
    pub fn parse(value: &str, locator: &R2BucketLocator) -> Result<Self, PlatformError> {
        if value
            .split('/')
            .any(|segment| segment == "." || segment == "..")
        {
            return Err(PlatformError::new(
                ErrorCode::R2KeyInvalid,
                "R2 object key contains a reserved path segment",
            ));
        }
        if value.len() > locator.max_user_key_bytes() {
            return Err(PlatformError::new(
                ErrorCode::R2KeyTooLarge,
                "R2 object key exceeds the physical provider-key budget",
            ));
        }
        Ok(Self(value.to_owned()))
    }

    /// Exact, unnormalized tenant string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Six HTTP metadata fields supported by the P0.5 facade.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct R2HttpMetadata {
    /// `Content-Type`.
    pub content_type: Option<String>,
    /// `Content-Language`.
    pub content_language: Option<String>,
    /// `Content-Disposition`.
    pub content_disposition: Option<String>,
    /// `Content-Encoding`.
    pub content_encoding: Option<String>,
    /// `Cache-Control`.
    pub cache_control: Option<String>,
    /// `Expires` as Unix milliseconds.
    pub cache_expiry: Option<i64>,
}

/// Single HTTP byte range accepted by P0.5.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct R2Range {
    /// First byte offset, absent for suffix ranges.
    pub offset: Option<u64>,
    /// Requested byte count, absent for open-ended or suffix ranges.
    pub length: Option<u64>,
    /// Last N bytes, mutually exclusive with offset/length.
    pub suffix: Option<u64>,
}

impl R2Range {
    pub(crate) fn header(self) -> Result<String, PlatformError> {
        match (self.offset, self.length, self.suffix) {
            (Some(offset), Some(length), None) if length > 0 => {
                let end = offset.checked_add(length - 1).ok_or_else(invalid_options)?;
                Ok(format!("bytes={offset}-{end}"))
            }
            (Some(offset), None, None) => Ok(format!("bytes={offset}-")),
            (None, Some(length), None) if length > 0 => Ok(format!("bytes=0-{}", length - 1)),
            (None, None, Some(suffix)) if suffix > 0 => Ok(format!("bytes=-{suffix}")),
            _ => Err(invalid_options()),
        }
    }
}

/// ETag/date read condition or atomic `ETag` write condition.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct R2Condition {
    /// At least one opaque `ETag` must match.
    #[serde(default)]
    pub etag_matches: Vec<String>,
    /// Every listed opaque `ETag` must differ.
    #[serde(default)]
    pub etag_does_not_match: Vec<String>,
    /// Object must not have been uploaded after this Unix millisecond.
    pub uploaded_before: Option<i64>,
    /// Object must have been uploaded after this Unix millisecond.
    pub uploaded_after: Option<i64>,
}

/// Host-validated put metadata and atomic condition.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct R2PutOptions {
    /// Standard HTTP metadata.
    pub http_metadata: R2HttpMetadata,
    /// Canonically ordered tenant metadata.
    pub custom_metadata: BTreeMap<String, String>,
    /// Optional atomic `ETag` condition.
    pub only_if: Option<R2Condition>,
    /// Optional caller-provided MD5 bytes.
    pub expected_md5: Option<[u8; 16]>,
}

/// Secure, replayable local file prepared before the single-part S3 PUT.
#[derive(Debug)]
pub struct R2UploadSource {
    /// Owned staging path opened by the typed store.
    pub path: PathBuf,
    /// Exact byte length.
    pub length: u64,
    /// MD5 calculated while staging.
    pub md5: [u8; 16],
    /// `UUIDv7` object version allocated once for this backend operation.
    pub version: String,
}

/// Tenant-visible object metadata DTO.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct R2ObjectMetadata {
    /// Exact unnormalized tenant key.
    pub key: String,
    /// Host-generated `UUIDv7` version.
    pub version: String,
    /// Full object size in bytes.
    pub size: u64,
    /// Opaque unquoted provider `ETag`.
    pub etag: String,
    /// RFC-compatible quoted `ETag` form.
    pub http_etag: String,
    /// Provider upload time in Unix milliseconds.
    pub uploaded: i64,
    /// Standard HTTP metadata.
    pub http_metadata: R2HttpMetadata,
    /// Canonical tenant metadata.
    pub custom_metadata: BTreeMap<String, String>,
    /// Returned byte range, when present.
    pub range: Option<R2Range>,
    /// MD5 hex computed from the staged bytes.
    pub md5: String,
    /// P0.5 supports only Standard storage class.
    pub storage_class: String,
}

/// Streaming object download returned after headers and metadata validate.
#[derive(Debug)]
pub struct R2Download {
    /// Trusted object metadata belonging to this body.
    pub metadata: R2ObjectMetadata,
    /// Provider byte stream. Consumers retain authorization pins separately.
    pub body: ByteStream,
}

/// `get()` result including condition-failed metadata without a body.
#[derive(Debug)]
pub enum R2GetResult {
    /// Object was absent.
    Missing,
    /// Condition failed and only metadata is returned.
    Precondition(R2ObjectMetadata),
    /// Condition passed and a streaming body is available.
    Body(R2Download),
}

/// Minimal list result before optional bounded metadata HEAD fanout.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct R2ListedObject {
    /// Exact logical key.
    pub key: String,
    /// Provider-reported size.
    pub size: u64,
    /// Opaque provider `ETag`.
    pub etag: String,
    /// Upload time in Unix milliseconds.
    pub uploaded: i64,
}

/// One provider-ordered logical bucket list page.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct R2ListPage {
    /// Provider-ordered object rows.
    pub objects: Vec<R2ListedObject>,
    /// Provider-ordered logical delimited prefixes.
    pub delimited_prefixes: Vec<String>,
    /// Opaque provider token retained only inside the host boundary.
    pub provider_token: Option<String>,
}

fn invalid_options() -> PlatformError {
    PlatformError::new(ErrorCode::R2InvalidOptions, "R2 options are invalid")
}
