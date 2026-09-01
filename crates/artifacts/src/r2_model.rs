//! Boundary types shared by the typed R2 object store and its callers.

use aws_sdk_s3::primitives::ByteStream;
use aws_sdk_s3::types::StorageClass;
use base64::Engine as _;
use md5::{Digest as _, Md5};
use open_compute_core::{ErrorCode, PlatformError, PlatformId, ResourceId};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;
use zeroize::{Zeroize, ZeroizeOnDrop};

/// Maximum UTF-8 bytes in one Cloudflare-compatible logical object key.
pub const R2_MAX_KEY_BYTES: usize = 1024;
/// Maximum keys accepted by one delete call.
pub const R2_MAX_DELETE_KEYS: usize = 1000;
/// Default and maximum list page size.
pub const R2_MAX_LIST_LIMIT: u16 = 1000;
/// Maximum canonical custom metadata bytes before base64 wrapping.
pub const R2_MAX_CUSTOM_METADATA_JSON_BYTES: usize = 4 * 1024;
/// Minimum size of every non-final multipart part.
pub const R2_MIN_MULTIPART_PART_BYTES: u64 = 5 * 1024 * 1024;
/// Maximum size of one multipart part.
pub const R2_MAX_MULTIPART_PART_BYTES: u64 = 5 * 1024 * 1024 * 1024;
/// Maximum completed multipart object size (5 TiB less one maximum-sized part).
pub const R2_MAX_MULTIPART_OBJECT_BYTES: u64 =
    5 * 1024 * 1024 * 1024 * 1024 - R2_MAX_MULTIPART_PART_BYTES;
/// Maximum part number and completed part count.
pub const R2_MAX_MULTIPART_PARTS: i32 = 10_000;

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

    /// Canonical immutable identity-marker key for this validated locator.
    #[must_use]
    pub fn identity_marker_key(&self) -> String {
        format!("{}meta/identity.json", self.physical_prefix)
    }
}

/// User object key validated without trimming, decoding, or normalization.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct UserObjectKey(String);

impl UserObjectKey {
    /// Validate the exact tenant key independently from its opaque provider mapping.
    pub fn parse(value: &str) -> Result<Self, PlatformError> {
        if value.len() > R2_MAX_KEY_BYTES {
            return Err(PlatformError::new(
                ErrorCode::R2KeyTooLarge,
                "R2 object key exceeds 1024 UTF-8 bytes",
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

/// One opaque, weak, or wildcard `ETag` from `onlyIf` or Headers.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum R2EtagMatch {
    /// RFC 7232 wildcard.
    Wildcard,
    /// Strong validator.
    Strong {
        /// Unquoted `ETag` value.
        value: String,
    },
    /// Weak validator.
    Weak {
        /// Unquoted `ETag` value.
        value: String,
    },
}

impl R2EtagMatch {
    fn strong_matches(&self, etag: &str) -> bool {
        match self {
            Self::Wildcard => true,
            Self::Strong { value } => value == etag,
            Self::Weak { .. } => false,
        }
    }

    fn weak_matches(&self, etag: &str) -> bool {
        match self {
            Self::Wildcard => true,
            Self::Strong { value } | Self::Weak { value } => value == etag,
        }
    }
}

/// ETag/date read or write condition, including Headers-form lists.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct R2Condition {
    /// At least one listed validator must match.
    #[serde(default)]
    pub etag_matches: Vec<R2EtagMatch>,
    /// Every listed validator must differ.
    #[serde(default)]
    pub etag_does_not_match: Vec<R2EtagMatch>,
    /// Object must not have been uploaded after this Unix millisecond.
    pub uploaded_before: Option<i64>,
    /// Object must have been uploaded after this Unix millisecond.
    pub uploaded_after: Option<i64>,
    /// Compare upload timestamps at whole-second resolution.
    #[serde(default)]
    pub seconds_granularity: bool,
    /// Apply RFC conditional-header precedence rather than `R2Conditional` field conjunction.
    #[serde(default)]
    pub http_headers: bool,
}

impl R2Condition {
    /// Whether a missing object satisfies this condition.
    #[must_use]
    pub fn matches_missing(&self) -> bool {
        self.etag_matches.is_empty()
            && (self.http_headers && !self.etag_matches.is_empty()
                || self.uploaded_before.is_none())
            && (self.http_headers && !self.etag_does_not_match.is_empty()
                || self.uploaded_after.is_none())
    }

    /// Whether one stored object satisfies this condition.
    #[must_use]
    pub fn matches_object(&self, etag: &str, uploaded: i64) -> bool {
        let uploaded = compare_time(uploaded, self.seconds_granularity);
        (self.etag_matches.is_empty()
            || self
                .etag_matches
                .iter()
                .any(|item| item.strong_matches(etag)))
            && self
                .etag_does_not_match
                .iter()
                .all(|item| !item.weak_matches(etag))
            && (self.http_headers && !self.etag_matches.is_empty()
                || self
                    .uploaded_before
                    .is_none_or(|time| uploaded <= compare_time(time, self.seconds_granularity)))
            && (self.http_headers && !self.etag_does_not_match.is_empty()
                || self
                    .uploaded_after
                    .is_none_or(|time| uploaded > compare_time(time, self.seconds_granularity)))
    }
}

fn compare_time(millis: i64, seconds: bool) -> i64 {
    if seconds {
        millis.div_euclid(1000)
    } else {
        millis
    }
}

/// Pinned Worker API storage class names.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub enum R2StorageClass {
    /// Default hot class.
    #[default]
    Standard,
    /// Infrequent-access class; no billing simulation.
    InfrequentAccess,
}

impl R2StorageClass {
    /// Parse a Worker API storage-class token.
    pub fn parse(value: &str) -> Result<Self, PlatformError> {
        match value {
            "Standard" => Ok(Self::Standard),
            "InfrequentAccess" => Ok(Self::InfrequentAccess),
            _ => Err(invalid_options()),
        }
    }

    /// Tenant-visible class name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Standard => "Standard",
            Self::InfrequentAccess => "InfrequentAccess",
        }
    }

    /// S3 storage-class token for the configured provider.
    #[must_use]
    pub fn s3(self) -> StorageClass {
        match self {
            Self::Standard => StorageClass::Standard,
            Self::InfrequentAccess => StorageClass::StandardIa,
        }
    }
}

/// Exactly one caller-supplied checksum algorithm.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum R2ChecksumAlgorithm {
    /// 16-byte MD5.
    Md5([u8; 16]),
    /// 20-byte SHA-1.
    Sha1([u8; 20]),
    /// 32-byte SHA-256.
    Sha256([u8; 32]),
    /// 48-byte SHA-384.
    Sha384([u8; 48]),
    /// 64-byte SHA-512.
    Sha512([u8; 64]),
}

/// All checksums computed from staged object bytes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct R2ComputedChecksums {
    /// MD5 of the staged bytes.
    pub md5: [u8; 16],
    /// SHA-1 of the staged bytes.
    pub sha1: [u8; 20],
    /// SHA-256 of the staged bytes.
    pub sha256: [u8; 32],
    /// SHA-384 of the staged bytes.
    pub sha384: [u8; 48],
    /// SHA-512 of the staged bytes.
    pub sha512: [u8; 64],
}

impl R2ComputedChecksums {
    pub(crate) fn exposed(&self, requested: Option<&R2ChecksumAlgorithm>) -> R2Checksums {
        let mut checksums = R2Checksums {
            md5: Some(hex::encode(self.md5)),
            ..R2Checksums::default()
        };
        match requested {
            None | Some(R2ChecksumAlgorithm::Md5(_)) => {}
            Some(R2ChecksumAlgorithm::Sha1(_)) => {
                checksums.sha1 = Some(hex::encode(self.sha1));
            }
            Some(R2ChecksumAlgorithm::Sha256(_)) => {
                checksums.sha256 = Some(hex::encode(self.sha256));
            }
            Some(R2ChecksumAlgorithm::Sha384(_)) => {
                checksums.sha384 = Some(hex::encode(self.sha384));
            }
            Some(R2ChecksumAlgorithm::Sha512(_)) => {
                checksums.sha512 = Some(hex::encode(self.sha512));
            }
        }
        checksums
    }

    /// Compare one caller-supplied algorithm against staged bytes.
    #[must_use]
    pub fn matches(&self, expected: &R2ChecksumAlgorithm) -> bool {
        match expected {
            R2ChecksumAlgorithm::Md5(value) => self.md5 == *value,
            R2ChecksumAlgorithm::Sha1(value) => self.sha1 == *value,
            R2ChecksumAlgorithm::Sha256(value) => self.sha256 == *value,
            R2ChecksumAlgorithm::Sha384(value) => self.sha384 == *value,
            R2ChecksumAlgorithm::Sha512(value) => self.sha512 == *value,
        }
    }
}

/// Hex-encoded checksums returned on object metadata.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct R2Checksums {
    /// MD5 hex.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub md5: Option<String>,
    /// SHA-1 hex.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha1: Option<String>,
    /// SHA-256 hex.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    /// SHA-384 hex.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha384: Option<String>,
    /// SHA-512 hex.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha512: Option<String>,
}

/// 32-byte SSE-C key. Plaintext never enters SQLite, logs, or public errors.
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct R2SsecKey {
    bytes: [u8; 32],
}

impl std::fmt::Debug for R2SsecKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("R2SsecKey").finish_non_exhaustive()
    }
}

impl PartialEq for R2SsecKey {
    fn eq(&self, other: &Self) -> bool {
        self.bytes == other.bytes
    }
}

impl Eq for R2SsecKey {}

impl R2SsecKey {
    /// Parse a 64-character lowercase or mixed hex key.
    pub fn parse_hex(value: &str) -> Result<Self, PlatformError> {
        if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(ssec_invalid());
        }
        let bytes = hex::decode(value).map_err(|_| ssec_invalid())?;
        Self::from_bytes(&bytes)
    }

    /// Parse a raw 32-byte key.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, PlatformError> {
        let bytes: [u8; 32] = bytes.try_into().map_err(|_| ssec_invalid())?;
        Ok(Self { bytes })
    }

    /// Lowercase hex form used on the private binding protocol.
    #[must_use]
    pub fn hex(&self) -> String {
        hex::encode(self.bytes)
    }

    /// Raw 32-byte key for AEAD sealing at the storage authority.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.bytes
    }

    /// Standard base64 of the key material for the S3 SSE-C header.
    #[must_use]
    pub fn base64(&self) -> String {
        base64::engine::general_purpose::STANDARD.encode(self.bytes)
    }

    /// S3 `x-amz-server-side-encryption-customer-key-MD5` value, also tenant `ssecKeyMd5`.
    #[must_use]
    pub fn md5_base64(&self) -> String {
        base64::engine::general_purpose::STANDARD.encode(Md5::digest(self.bytes))
    }
}

/// Host-validated put metadata, condition, checksum, class, and SSE-C.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct R2PutOptions {
    /// Standard HTTP metadata.
    pub http_metadata: R2HttpMetadata,
    /// Canonically ordered tenant metadata.
    pub custom_metadata: BTreeMap<String, String>,
    /// Optional atomic condition.
    pub only_if: Option<R2Condition>,
    /// Optional single caller-supplied checksum.
    pub checksum: Option<R2ChecksumAlgorithm>,
    /// Worker API storage class.
    pub storage_class: R2StorageClass,
    /// Optional SSE-C key for this mutation.
    pub ssec: Option<R2SsecKey>,
}

/// Multipart create options excluding body.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct R2MultipartCreateOptions {
    /// Standard HTTP metadata retained until complete.
    pub http_metadata: R2HttpMetadata,
    /// Canonically ordered tenant metadata.
    pub custom_metadata: BTreeMap<String, String>,
    /// Worker API storage class.
    pub storage_class: R2StorageClass,
    /// Optional SSE-C key for the whole upload.
    pub ssec: Option<R2SsecKey>,
}

/// One completed part returned to the tenant.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct R2UploadedPart {
    /// Part number in `1..=10000`.
    pub part_number: i32,
    /// Provider part `ETag`.
    pub etag: String,
}

/// Secure, replayable local file prepared before PUT or `UploadPart`.
#[derive(Debug)]
pub struct R2UploadSource {
    /// Owned staging path opened by the typed store.
    pub path: PathBuf,
    /// Exact byte length.
    pub length: u64,
    /// Checksums calculated while staging.
    pub checksums: R2ComputedChecksums,
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
    /// Standard HTTP metadata, omitted from unincluded list entries.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub http_metadata: Option<R2HttpMetadata>,
    /// Canonical tenant metadata, omitted from unincluded list entries.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub custom_metadata: Option<BTreeMap<String, String>>,
    /// Returned byte range, when present.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub range: Option<R2Range>,
    /// Stored checksums as lowercase hex.
    pub checksums: R2Checksums,
    /// Worker API storage class.
    pub storage_class: String,
    /// Base64 MD5 of the SSE-C key when the object is encrypted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ssec_key_md5: Option<String>,
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

pub(crate) fn invalid_options() -> PlatformError {
    PlatformError::new(ErrorCode::R2InvalidOptions, "R2 options are invalid")
}

pub(crate) fn ssec_invalid() -> PlatformError {
    PlatformError::new(
        ErrorCode::R2SsecInvalid,
        "R2 SSE-C key is invalid or does not match the object",
    )
}

pub(crate) fn checksum_mismatch() -> PlatformError {
    PlatformError::new(
        ErrorCode::R2ChecksumMismatch,
        "R2 checksum does not match staged object bytes",
    )
}

pub(crate) fn multipart_invalid() -> PlatformError {
    PlatformError::new(
        ErrorCode::R2MultipartInvalid,
        "R2 multipart upload is invalid",
    )
}
