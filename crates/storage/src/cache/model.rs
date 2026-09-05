//! Canonical cache identity and response metadata.

use open_compute_core::{AccountId, ErrorCode, PlatformError, VersionId, WorkerId};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::str::FromStr;

/// Public cache behavior surface.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CacheSurface {
    /// Version-configured automatic response cache.
    Automatic,
    /// Explicit `caches.default` namespace.
    CacheApiDefault,
    /// Explicit `caches.open(name)` namespace.
    CacheApiNamed,
}

/// Cacheable HTTP request method.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum CacheMethod {
    /// GET request or stored representation.
    Get,
    /// HEAD lookup, keyed to the corresponding GET representation.
    Head,
}

impl CacheMethod {
    pub(crate) const fn key_class(self) -> &'static str {
        "GET"
    }
}

/// Fully scoped logical cache key.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CacheIdentity {
    /// Account isolation boundary.
    pub account_id: AccountId,
    /// Stable logical Worker identity.
    pub worker_id: WorkerId,
    /// Automatic or explicit API surface.
    pub surface: CacheSurface,
    /// Automatic-cache entrypoint, absent for Cache API namespaces.
    pub entrypoint: Option<String>,
    /// Version ID or `shared` for automatic cache; always `shared` for Cache API.
    pub version_scope: String,
    /// Named-cache namespace, present only for `cache_api_named`.
    pub cache_name: Option<String>,
    /// Canonical absolute HTTP(S) URL.
    pub canonical_url: String,
    /// GET or HEAD request class.
    pub method: CacheMethod,
}

impl CacheIdentity {
    pub(crate) fn validate(&self, max_url: usize, max_name: usize) -> Result<(), PlatformError> {
        if self.canonical_url.len() > max_url {
            return Err(key_invalid());
        }
        let url = url::Url::parse(&self.canonical_url).map_err(|_| key_invalid())?;
        if !matches!(url.scheme(), "http" | "https")
            || url.fragment().is_some()
            || canonical_url(url)? != self.canonical_url
        {
            return Err(key_invalid());
        }
        match self.surface {
            CacheSurface::Automatic => {
                let entrypoint = self.entrypoint.as_deref().ok_or_else(key_invalid)?;
                if !valid_entrypoint(entrypoint)
                    || self.cache_name.is_some()
                    || (self.version_scope != "shared"
                        && VersionId::from_str(&self.version_scope).is_err())
                {
                    return Err(key_invalid());
                }
            }
            CacheSurface::CacheApiDefault => {
                if self.entrypoint.is_some()
                    || self.cache_name.is_some()
                    || self.version_scope != "shared"
                {
                    return Err(key_invalid());
                }
            }
            CacheSurface::CacheApiNamed => {
                let name = self.cache_name.as_deref().ok_or_else(key_invalid)?;
                if self.entrypoint.is_some()
                    || self.version_scope != "shared"
                    || name.is_empty()
                    || name.len() > max_name
                    || name.bytes().any(|byte| byte.is_ascii_control())
                {
                    return Err(key_invalid());
                }
            }
        }
        Ok(())
    }

    pub(crate) fn canonical_bytes(&self) -> Result<Vec<u8>, PlatformError> {
        let mut canonical = self.clone();
        canonical.method = CacheMethod::Get;
        serde_json::to_vec(&canonical).map_err(|_| protocol_error())
    }

    pub(crate) fn base_hash(&self) -> Result<[u8; 32], PlatformError> {
        let mut hasher = Sha256::new();
        hasher.update(b"open-compute/response-cache-key/v1\0");
        hasher.update(self.canonical_bytes()?);
        hasher.update([0]);
        hasher.update(self.method.key_class().as_bytes());
        Ok(hasher.finalize().into())
    }
}

/// One canonical response header.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CacheHeader {
    /// Lowercase HTTP field name.
    pub name: String,
    /// Field value as exposed by the Fetch `Headers` API.
    pub value: String,
}

/// Immutable object-body identity referenced by cache metadata.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CacheBodyRef {
    /// Lowercase SHA-256.
    pub sha256: String,
    /// Exact body byte length.
    pub size: u64,
}

/// Validated cached HTTP response metadata and body reference.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CacheStoredResponse {
    /// HTTP response status.
    pub status: u16,
    /// Canonical response headers with internal fields removed.
    pub headers: Vec<CacheHeader>,
    /// Immutable response body reference.
    pub body: CacheBodyRef,
    /// Sorted lowercase Vary field names.
    pub vary: Vec<String>,
    /// Sorted lowercase cache tags.
    pub tags: Vec<String>,
    /// Exclusive freshness deadline.
    pub fresh_until_ms: i64,
    /// Exclusive stale-while-revalidate deadline.
    pub stale_while_revalidate_until_ms: i64,
    /// Exclusive stale-if-error deadline.
    pub stale_if_error_until_ms: i64,
    /// Metadata generation committed with the entry.
    pub generation: u64,
}

/// Cache lookup state returned by the metadata authority.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum CacheLookupStatus {
    /// No matching entry exists.
    Miss,
    /// A fresh entry is available.
    Hit,
    /// A stale entry is served while this request owns refresh.
    Updating,
    /// A stale entry is served while another request owns refresh.
    Stale,
    /// A stale candidate may be used only if origin execution fails.
    StaleIfError,
    /// An expired entry was observed and treated as a miss.
    Expired,
}

/// One lookup result and its purge/refresh fencing material.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CacheLookup {
    /// Lookup classification.
    pub status: CacheLookupStatus,
    /// Matching stored response, if usable.
    pub response: Option<CacheStoredResponse>,
    /// Current Worker-wide purge fence captured by this lookup.
    pub fence_generation: u64,
    /// Refresh lease token only for the updating owner.
    pub refresh_token: Option<String>,
}

/// Complete metadata transaction input after the body reached immutable object storage.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CachePut {
    /// Scoped logical key.
    pub identity: CacheIdentity,
    /// Canonical request headers used for Vary.
    pub request_headers: BTreeMap<String, String>,
    /// Stored response metadata and body identity.
    pub response: CacheStoredResponse,
    /// Fence captured before body upload.
    pub expected_fence_generation: u64,
    /// Optional refresh-owner lease token.
    pub refresh_token: Option<String>,
    /// Commit wall time.
    pub now_ms: i64,
}

/// Worker-local purge selector.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CachePurge {
    /// Canonical lowercase tags.
    #[serde(default)]
    pub tags: Vec<String>,
    /// Absolute URL or URL-path prefixes.
    #[serde(default)]
    pub path_prefixes: Vec<String>,
    /// Delete all entries for the Worker.
    #[serde(default)]
    pub purge_everything: bool,
}

pub(crate) fn validate_headers(
    headers: &[CacheHeader],
    max_bytes: usize,
) -> Result<(), PlatformError> {
    let mut total = 0_usize;
    for header in headers {
        if header.name.is_empty()
            || header.name.len() > 128
            || header.name.bytes().any(|byte| {
                !byte.is_ascii_lowercase() && !byte.is_ascii_digit() && !matches!(byte, b'-')
            })
            || header
                .value
                .bytes()
                .any(|byte| matches!(byte, b'\r' | b'\n' | 0))
            || is_internal_or_hop_by_hop(&header.name)
        {
            return Err(protocol_error());
        }
        total = total
            .checked_add(
                header
                    .name
                    .len()
                    .saturating_add(header.value.len())
                    .saturating_add(4),
            )
            .ok_or_else(limit_error)?;
    }
    if total > max_bytes {
        return Err(limit_error());
    }
    Ok(())
}

pub(crate) fn validate_request_headers(
    headers: &BTreeMap<String, String>,
    max_bytes: usize,
) -> Result<(), PlatformError> {
    validate_headers(
        &headers
            .iter()
            .map(|(name, value)| CacheHeader {
                name: name.clone(),
                value: value.clone(),
            })
            .collect::<Vec<_>>(),
        max_bytes,
    )
}

pub(crate) fn validate_vary(vary: &[String]) -> Result<(), PlatformError> {
    if vary.len() > 32 {
        return Err(limit_error());
    }
    let mut prior: Option<&str> = None;
    for name in vary {
        if name == "*"
            || name.is_empty()
            || name.len() > 128
            || name
                .bytes()
                .any(|byte| !byte.is_ascii_lowercase() && !byte.is_ascii_digit() && byte != b'-')
            || prior.is_some_and(|value| value >= name.as_str())
        {
            return Err(protocol_error());
        }
        prior = Some(name);
    }
    Ok(())
}

pub(crate) fn validate_tags(tags: &[String], maximum: usize) -> Result<(), PlatformError> {
    if tags.len() > maximum {
        return Err(limit_error());
    }
    let mut seen = BTreeSet::new();
    for tag in tags {
        if tag.is_empty()
            || tag.len() > 128
            || tag.bytes().any(|byte| byte.is_ascii_control())
            || tag.to_ascii_lowercase() != *tag
            || !seen.insert(tag)
        {
            return Err(protocol_error());
        }
    }
    Ok(())
}

pub(crate) fn vary_fingerprint(vary: &[String], headers: &BTreeMap<String, String>) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"open-compute/cache-vary/v1\0");
    for name in vary {
        hasher.update((name.len() as u64).to_be_bytes());
        hasher.update(name.as_bytes());
        let value = headers.get(name).map_or("", String::as_str);
        hasher.update((value.len() as u64).to_be_bytes());
        hasher.update(value.as_bytes());
    }
    hasher.finalize().into()
}

pub(crate) fn canonical_url(mut url: url::Url) -> Result<String, PlatformError> {
    url.set_fragment(None);
    let default_port = matches!(
        (url.scheme(), url.port()),
        ("http", Some(80)) | ("https", Some(443))
    );
    if default_port {
        url.set_port(None).map_err(|()| key_invalid())?;
    }
    if let Some(host) = url.host_str() {
        let lower = host.to_ascii_lowercase();
        url.set_host(Some(&lower)).map_err(|_| key_invalid())?;
    }
    Ok(url.into())
}

fn valid_entrypoint(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_alphabetic() || matches!(byte, b'_' | b'$'))
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'$'))
}

fn is_internal_or_hop_by_hop(name: &str) -> bool {
    name.starts_with("x-open-compute-")
        || matches!(
            name,
            "connection"
                | "keep-alive"
                | "proxy-authenticate"
                | "proxy-authorization"
                | "te"
                | "trailer"
                | "transfer-encoding"
                | "upgrade"
        )
}

pub(crate) fn key_invalid() -> PlatformError {
    PlatformError::new(ErrorCode::CacheKeyInvalid, "cache key is invalid")
}

pub(crate) fn protocol_error() -> PlatformError {
    PlatformError::new(
        ErrorCode::CacheProtocolError,
        "cache metadata protocol is invalid",
    )
}

pub(crate) fn limit_error() -> PlatformError {
    PlatformError::new(ErrorCode::CacheLimitExceeded, "cache limit was exceeded")
}

pub(crate) fn corrupt() -> PlatformError {
    PlatformError::new(
        ErrorCode::CacheCorrupt,
        "cache database failed an integrity invariant",
    )
}
