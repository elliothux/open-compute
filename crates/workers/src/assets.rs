//! Canonical static-asset manifests and immutable routing configuration.

use crate::descriptor::validate_env_name;
use open_compute_artifacts::{ARTIFACT_KEY_VERSION, ArtifactRef};
use open_compute_core::{ErrorCode, PlatformError};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

mod handler;
pub use handler::{AssetRequest, AssetResponsePlan, plan_asset_response};

/// Maximum number of logical asset paths in one deployment.
pub const MAX_ASSET_FILES: usize = 20_000;
/// Maximum bytes in one asset.
pub const MAX_ASSET_FILE_BYTES: u64 = 25 * 1024 * 1024;
/// Maximum logical bytes across one deployment.
pub const MAX_ASSET_TOTAL_BYTES: u64 = 512 * 1024 * 1024;
/// Maximum canonical manifest size.
pub const MAX_ASSET_MANIFEST_BYTES: usize = 16 * 1024 * 1024;
/// Maximum `run_worker_first` rule count.
pub const MAX_ASSET_ROUTING_RULES: usize = 100;

/// One immutable logical path in an asset manifest.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AssetEntryV1 {
    /// Canonical URL path beginning with exactly one slash.
    pub path: String,
    /// Lowercase SHA-256 of the original file bytes.
    pub sha256: String,
    /// Exact file length.
    pub size: u64,
    /// Deterministic response media type.
    pub content_type: String,
}

impl AssetEntryV1 {
    /// Convert this validated entry to the shared physical artifact identity.
    pub fn artifact_ref(&self) -> Result<ArtifactRef, PlatformError> {
        ArtifactRef::new(ARTIFACT_KEY_VERSION, &self.sha256, self.size)
            .map_err(|_| manifest_invalid())
    }
}

/// Canonical, path-sorted manifest for one immutable deployment.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AssetManifestV1 {
    /// Manifest schema. The Day1 implementation accepts exactly one schema.
    pub schema_version: u32,
    /// Byte-wise path-sorted logical entries.
    pub entries: Vec<AssetEntryV1>,
}

impl AssetManifestV1 {
    /// Validate a complete manifest and all fixed quotas.
    pub fn validate(&self) -> Result<(), PlatformError> {
        if self.schema_version != 1 || self.entries.is_empty() {
            return Err(manifest_invalid());
        }
        if self.entries.len() > MAX_ASSET_FILES {
            return Err(asset_limit_exceeded());
        }
        let mut prior: Option<&str> = None;
        let mut total = 0_u64;
        for entry in &self.entries {
            validate_asset_path(&entry.path)?;
            validate_sha256(&entry.sha256)?;
            if entry.size > MAX_ASSET_FILE_BYTES {
                return Err(asset_limit_exceeded());
            }
            if entry.content_type.is_empty()
                || entry.content_type.len() > 255
                || !entry.content_type.is_ascii()
                || entry
                    .content_type
                    .bytes()
                    .any(|byte| byte.is_ascii_control())
            {
                return Err(manifest_invalid());
            }
            if prior.is_some_and(|value| value.as_bytes() >= entry.path.as_bytes()) {
                return Err(manifest_invalid());
            }
            prior = Some(&entry.path);
            total = total
                .checked_add(entry.size)
                .ok_or_else(asset_limit_exceeded)?;
            if total > MAX_ASSET_TOTAL_BYTES {
                return Err(asset_limit_exceeded());
            }
        }
        if self.canonical_bytes_unchecked()?.len() > MAX_ASSET_MANIFEST_BYTES {
            return Err(asset_limit_exceeded());
        }
        Ok(())
    }

    /// Canonical UTF-8 JSON bytes used as the persisted manifest object.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, PlatformError> {
        self.validate()?;
        self.canonical_bytes_unchecked()
    }

    /// SHA-256 of the canonical manifest bytes.
    pub fn sha256(&self) -> Result<[u8; 32], PlatformError> {
        Ok(Sha256::digest(self.canonical_bytes()?).into())
    }

    /// Exact logical byte total before physical object deduplication.
    pub fn total_bytes(&self) -> Result<u64, PlatformError> {
        self.validate()?;
        self.entries.iter().try_fold(0_u64, |total, entry| {
            total
                .checked_add(entry.size)
                .ok_or_else(asset_limit_exceeded)
        })
    }

    fn canonical_bytes_unchecked(&self) -> Result<Vec<u8>, PlatformError> {
        serde_json::to_vec(self).map_err(|_| manifest_invalid())
    }
}

/// Worker-first selection for the deployment's default HTTP route.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RunWorkerFirst {
    /// Apply one decision to every request.
    All(bool),
    /// Ordered glob-only rules. Exclusions begin with `!` and win globally.
    Rules(Vec<String>),
}

impl Default for RunWorkerFirst {
    fn default() -> Self {
        Self::All(false)
    }
}

/// Static HTML path normalization mode.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum HtmlHandling {
    /// Match the upstream default canonical redirects and index lookup.
    #[default]
    AutoTrailingSlash,
    /// Canonicalize extensionless HTML to a trailing slash.
    ForceTrailingSlash,
    /// Canonicalize extensionless HTML without a trailing slash.
    DropTrailingSlash,
    /// Serve only exact manifest paths.
    None,
}

/// Static not-found behavior.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum NotFoundHandling {
    /// Return an empty 404 response.
    #[default]
    None,
    /// Select the nearest ancestor `404.html`.
    #[serde(rename = "404-page")]
    Page404,
    /// Return the root `index.html` for eligible navigation requests.
    SinglePageApplication,
}

/// One deterministic `_headers` mutation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AssetHeaderOperation {
    /// Lowercase HTTP field name.
    pub name: String,
    /// Replacement value, or `None` to remove the field.
    pub value: Option<String>,
}

/// One parsed `_headers` path or absolute-URL rule.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AssetHeaderRule {
    /// Bounded glob/placeholder pattern.
    pub pattern: String,
    /// Ordered mutations applied when this rule matches.
    pub operations: Vec<AssetHeaderOperation>,
}

/// One parsed `_redirects` redirect or same-site rewrite.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AssetRedirectRule {
    /// Source glob/placeholder pattern.
    pub from: String,
    /// Destination path or absolute URL.
    pub to: String,
    /// HTTP redirect status or `200` for an internal rewrite.
    pub status: u16,
}

/// Canonical route behavior frozen with a deployment.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AssetRoutingConfigV1 {
    /// Routing schema. The Day1 implementation accepts exactly one schema.
    pub schema_version: u32,
    /// Optional tenant environment binding name.
    #[serde(default)]
    pub binding: Option<String>,
    /// Worker-first mode for default HTTP fetches.
    #[serde(default)]
    pub run_worker_first: RunWorkerFirst,
    /// HTML path handling.
    #[serde(default)]
    pub html_handling: HtmlHandling,
    /// Missing-path handling.
    #[serde(default)]
    pub not_found_handling: NotFoundHandling,
    /// Parsed `_headers` rules.
    #[serde(default)]
    pub headers: Vec<AssetHeaderRule>,
    /// Parsed `_redirects` rules.
    #[serde(default)]
    pub redirects: Vec<AssetRedirectRule>,
}

impl AssetRoutingConfigV1 {
    /// Validate canonical names, fixed parser limits, and supported status codes.
    pub fn validate(&self) -> Result<(), PlatformError> {
        if self.schema_version != 1 || self.headers.len() > 100 || self.redirects.len() > 2_000 {
            return Err(config_unsupported());
        }
        if let Some(binding) = &self.binding {
            validate_env_name(binding)?;
            if binding.len() > 64 {
                return Err(config_unsupported());
            }
        }
        if let RunWorkerFirst::Rules(rules) = &self.run_worker_first {
            if rules.is_empty() || rules.len() > MAX_ASSET_ROUTING_RULES {
                return Err(config_unsupported());
            }
            for rule in rules {
                let path = rule.strip_prefix('!').unwrap_or(rule);
                if path.len() > 2_048 || !path.starts_with('/') || invalid_text(path) {
                    return Err(config_unsupported());
                }
            }
        }
        for rule in &self.headers {
            validate_rule_text(&rule.pattern)?;
            validate_match_pattern(&rule.pattern)?;
            if rule.operations.is_empty() || rule.operations.len() > 100 {
                return Err(config_unsupported());
            }
            let mut names = BTreeSet::new();
            for operation in &rule.operations {
                if operation.name.is_empty()
                    || operation.name.len() > 128
                    || operation.name != operation.name.to_ascii_lowercase()
                    || !operation.name.bytes().all(|byte| {
                        byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&byte)
                    })
                    || !names.insert(operation.name.as_str())
                    || operation.value.as_ref().is_some_and(|value| {
                        value.len() > 4_096
                            || value.bytes().any(|byte| matches!(byte, b'\r' | b'\n' | 0))
                    })
                {
                    return Err(config_unsupported());
                }
            }
        }
        for rule in &self.redirects {
            validate_rule_text(&rule.from)?;
            validate_match_pattern(&rule.from)?;
            if rule.to.is_empty()
                || rule.to.len() > 2_048
                || invalid_text(&rule.to)
                || !matches!(rule.status, 200 | 301 | 302 | 303 | 307 | 308)
                || (rule.status == 200 && !rule.to.starts_with('/'))
            {
                return Err(config_unsupported());
            }
        }
        Ok(())
    }

    /// Canonical JSON bytes included in the deployment descriptor.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, PlatformError> {
        self.validate()?;
        serde_json::to_vec(self).map_err(|_| config_unsupported())
    }
}

/// Static-asset content supplied to the unified deployment pipeline.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeploymentAssets {
    /// Canonical manifest whose objects are already uploaded or uploaded by the caller.
    pub manifest: AssetManifestV1,
    /// Immutable route and optional binding behavior.
    pub routing: AssetRoutingConfigV1,
}

/// Validate one canonical manifest URL path.
pub fn validate_asset_path(path: &str) -> Result<(), PlatformError> {
    if path.len() < 2
        || path.len() > 2_048
        || !path.starts_with('/')
        || path.starts_with("//")
        || path.ends_with('/')
        || path.contains('\\')
        || invalid_text(path)
        || !path.is_ascii()
        || path
            .split('/')
            .skip(1)
            .any(|segment| !canonical_asset_segment(segment))
    {
        return Err(PlatformError::new(
            ErrorCode::AssetPathInvalid,
            "asset manifest path is invalid",
        ));
    }
    Ok(())
}

fn canonical_asset_segment(segment: &str) -> bool {
    if segment.is_empty() {
        return false;
    }
    let bytes = segment.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut offset = 0;
    while offset < bytes.len() {
        if bytes[offset] == b'%' {
            if offset + 2 >= bytes.len()
                || !bytes[offset + 1].is_ascii_hexdigit()
                || !bytes[offset + 2].is_ascii_hexdigit()
                || bytes[offset + 1].is_ascii_lowercase()
                || bytes[offset + 2].is_ascii_lowercase()
            {
                return false;
            }
            let high = hex_value(bytes[offset + 1]);
            let low = hex_value(bytes[offset + 2]);
            decoded.push((high << 4) | low);
            offset += 3;
        } else {
            decoded.push(bytes[offset]);
            offset += 1;
        }
    }
    let Ok(value) = std::str::from_utf8(&decoded) else {
        return false;
    };
    if matches!(value, "." | "..") || value.contains('/') || value.contains('\\') {
        return false;
    }
    encode_uri_component(value) == segment
}

fn hex_value(byte: u8) -> u8 {
    match byte {
        b'0'..=b'9' => byte - b'0',
        b'A'..=b'F' => byte - b'A' + 10,
        _ => 0,
    }
}

fn encode_uri_component(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.as_bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.!~*'()".contains(byte) {
            encoded.push(char::from(*byte));
        } else {
            encoded.push('%');
            encoded.push(char::from(b"0123456789ABCDEF"[usize::from(byte >> 4)]));
            encoded.push(char::from(b"0123456789ABCDEF"[usize::from(byte & 0x0f)]));
        }
    }
    encoded
}

fn validate_sha256(value: &str) -> Result<(), PlatformError> {
    if value.len() != 64
        || value
            .bytes()
            .any(|byte| !byte.is_ascii_hexdigit() || byte.is_ascii_uppercase())
    {
        return Err(manifest_invalid());
    }
    Ok(())
}

fn validate_rule_text(value: &str) -> Result<(), PlatformError> {
    if value.is_empty() || value.len() > 2_048 || invalid_text(value) {
        return Err(config_unsupported());
    }
    Ok(())
}

fn validate_match_pattern(value: &str) -> Result<(), PlatformError> {
    if value.matches('*').count() > 1 {
        return Err(config_unsupported());
    }
    for (offset, _) in value.match_indices(':') {
        let token = &value[offset + 1..];
        if token.starts_with('/') {
            continue;
        }
        let name = token
            .split(|character: char| !character.is_ascii_alphanumeric() && character != '_')
            .next()
            .unwrap_or_default();
        if name.is_empty()
            || !name
                .bytes()
                .next()
                .is_some_and(|byte| byte.is_ascii_alphabetic())
        {
            return Err(config_unsupported());
        }
    }
    Ok(())
}

fn invalid_text(value: &str) -> bool {
    value
        .chars()
        .any(|character| character == '\0' || character.is_control())
}

fn manifest_invalid() -> PlatformError {
    PlatformError::new(
        ErrorCode::AssetManifestInvalid,
        "asset manifest is not canonical",
    )
}

fn asset_limit_exceeded() -> PlatformError {
    PlatformError::new(
        ErrorCode::AssetLimitExceeded,
        "asset deployment exceeds a fixed limit",
    )
}

fn config_unsupported() -> PlatformError {
    PlatformError::new(
        ErrorCode::AssetConfigUnsupported,
        "asset routing configuration is unsupported",
    )
}

#[cfg(test)]
#[path = "assets_tests.rs"]
mod tests;
