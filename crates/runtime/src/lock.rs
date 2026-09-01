//! Strict current Day1 `workerd.lock.json` schema. Obsolete fields are rejected.

use crate::fsutil::{
    MAX_LOCK_BYTES, parse_sha256_hex, read_regular_nofollow_bounded, require_absolute,
    require_regular_file,
};
use open_compute_core::{ErrorCode, PlatformError};
use serde::de::{self, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::{Debug, Formatter};
use std::path::Path;
use url::Url;

const SCHEMA_VERSION: u32 = 1;
const TOKEN_PLACEHOLDER: &str = "__OPEN_COMPUTE_INTERNAL_TOKEN__";

/// Pinned workerd release lock.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RuntimeLock {
    /// Schema version. Must be 1.
    #[serde(rename = "schemaVersion")]
    pub schema_version: u32,
    /// Release tag, for example `v1.20260830.1`.
    pub release: String,
    /// workerd git revision that produced this release.
    pub revision: String,
    /// Exact `workerd --version` stdout, trimmed.
    #[serde(rename = "expectedVersionOutput")]
    pub expected_version_output: String,
    /// Internal tenant compatibility date mixed into every isolate.
    #[serde(rename = "effectiveCompatibilityDate")]
    pub effective_compatibility_date: String,
    /// Platform-internal flags required to enable the pinned tenant surface.
    #[serde(rename = "requiredCompatibilityFlags")]
    pub required_compatibility_flags: Vec<String>,
    /// System-Worker-only flags. Never tenant configuration or capabilities.
    #[serde(rename = "systemCompatibilityFlags")]
    pub system_compatibility_flags: Vec<String>,
    /// Required process flags, each starting with `--`.
    #[serde(rename = "processFlags")]
    pub process_flags: Vec<String>,
    /// Pinned `@cloudflare/workers-types` identity.
    #[serde(rename = "workersTypes")]
    pub workers_types: WorkersTypesPin,
    /// Pinned workers-sdk revision and tool versions.
    #[serde(rename = "workersSdk")]
    pub workers_sdk: WorkersSdkPin,
    /// OS/arch target map.
    pub targets: BTreeMap<String, RuntimeTarget>,
}

/// Immutable `@cloudflare/workers-types` pin.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WorkersTypesPin {
    /// npm package version, for example `5.20260830.1`.
    pub version: String,
    /// Upstream gitHead for this workers-types release.
    #[serde(rename = "gitHead")]
    pub git_head: String,
    /// SHA-256 of the npm package tarball.
    #[serde(rename = "packageSha256")]
    pub package_sha256: String,
    /// SHA-256 of the stable declaration AST source (`index.d.ts`).
    #[serde(rename = "astSha256")]
    pub ast_sha256: String,
}

/// Immutable workers-sdk pin.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WorkersSdkPin {
    /// workers-sdk git revision.
    pub revision: String,
    /// Wrangler package version from that revision.
    #[serde(rename = "wranglerVersion")]
    pub wrangler_version: String,
    /// Vite plugin package version from that revision.
    #[serde(rename = "vitePluginVersion")]
    pub vite_plugin_version: String,
}

/// Per-target official archive and binary hashes.
#[derive(Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RuntimeTarget {
    /// Archive file name.
    #[serde(rename = "archiveName")]
    pub archive_name: String,
    /// Official HTTPS download URL.
    #[serde(rename = "archiveUrl")]
    pub archive_url: String,
    /// SHA-256 of the compressed archive.
    #[serde(rename = "archiveSha256")]
    pub archive_sha256: String,
    /// SHA-256 of the decompressed binary.
    #[serde(rename = "binarySha256")]
    pub binary_sha256: String,
}

impl Debug for RuntimeTarget {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RuntimeTarget")
            .field("archive_name", &self.archive_name)
            .field("archive_url", &self.archive_url)
            .field("archive_sha256", &self.archive_sha256)
            .field("binary_sha256", &self.binary_sha256)
            .finish()
    }
}

impl RuntimeLock {
    /// Parse lock JSON from bytes. Unknown fields, duplicate keys, and schemas are rejected.
    pub fn parse(bytes: &[u8]) -> Result<Self, PlatformError> {
        if bytes.len() as u64 > MAX_LOCK_BYTES {
            return Err(PlatformError::new(
                ErrorCode::RuntimeInvalid,
                "workerd lock exceeds the size bound",
            ));
        }
        let value: NoDupValue = serde_json::from_slice(bytes).map_err(|_| {
            PlatformError::new(ErrorCode::RuntimeInvalid, "invalid workerd lock JSON")
        })?;
        let lock: Self = serde_json::from_value(value.0).map_err(|_| {
            PlatformError::new(ErrorCode::RuntimeInvalid, "invalid workerd lock JSON")
        })?;
        lock.validate()?;
        Ok(lock)
    }

    fn validate(&self) -> Result<(), PlatformError> {
        if self.schema_version != SCHEMA_VERSION {
            return Err(PlatformError::new(
                ErrorCode::RuntimeInvalid,
                "unsupported workerd lock schema version",
            ));
        }
        require_nonempty(&self.release, "release")?;
        require_git_sha(&self.revision)?;
        require_nonempty(&self.expected_version_output, "expectedVersionOutput")?;
        require_compat_date(&self.effective_compatibility_date)?;
        if self.process_flags.is_empty() {
            return Err(PlatformError::new(
                ErrorCode::RuntimeInvalid,
                "processFlags must not be empty",
            ));
        }
        let mut seen_flags = BTreeSet::new();
        for flag in &self.process_flags {
            if !flag.starts_with("--")
                || flag.len() < 3
                || flag.bytes().any(|b| b.is_ascii_whitespace() || b == b'=')
            {
                return Err(PlatformError::new(
                    ErrorCode::RuntimeInvalid,
                    "process flag is malformed",
                ));
            }
            if !seen_flags.insert(flag) {
                return Err(PlatformError::new(
                    ErrorCode::RuntimeInvalid,
                    "process flag is duplicated",
                ));
            }
        }
        let required = require_compat_flags(&self.required_compatibility_flags)?;
        let system = require_compat_flags(&self.system_compatibility_flags)?;
        if !required.is_disjoint(&system) {
            return Err(PlatformError::new(
                ErrorCode::RuntimeInvalid,
                "required and system compatibility flags must be disjoint",
            ));
        }
        self.workers_types.validate(&self.revision)?;
        self.workers_sdk.validate()?;
        if self.targets.is_empty() {
            return Err(PlatformError::new(
                ErrorCode::RuntimeInvalid,
                "workerd lock must list at least one target",
            ));
        }
        for (name, target) in &self.targets {
            validate_target_name(name)?;
            target.validate(&self.release, name)?;
        }
        Ok(())
    }

    /// Select the lock target for this process OS/arch.
    pub fn current_target(&self) -> Result<(&str, &RuntimeTarget), PlatformError> {
        let name = current_target_name()?;
        let target = self.targets.get(name).ok_or_else(|| {
            PlatformError::new(
                ErrorCode::RuntimeInvalid,
                "workerd lock does not include the current OS/arch target",
            )
        })?;
        Ok((name, target))
    }

    /// Token placeholder embedded in the packaged Cap'n Proto template.
    #[must_use]
    pub const fn token_placeholder() -> &'static str {
        TOKEN_PLACEHOLDER
    }
}

impl RuntimeTarget {
    fn validate(&self, release: &str, target_name: &str) -> Result<(), PlatformError> {
        require_nonempty(&self.archive_name, "archiveName")?;
        if self.archive_name.contains('/')
            || self.archive_name.contains('\\')
            || self.archive_name.contains('\0')
        {
            return Err(PlatformError::new(
                ErrorCode::RuntimeInvalid,
                "archive name must be a file name",
            ));
        }
        let url = Url::parse(&self.archive_url).map_err(|_| {
            PlatformError::new(ErrorCode::RuntimeInvalid, "archive URL is malformed")
        })?;
        if url.scheme() != "https" {
            return Err(PlatformError::new(
                ErrorCode::RuntimeInvalid,
                "archive URL must be https",
            ));
        }
        if url.username() != "" || url.password().is_some() {
            return Err(PlatformError::new(
                ErrorCode::RuntimeInvalid,
                "archive URL must not contain credentials",
            ));
        }
        if url.host_str() != Some("github.com") {
            return Err(PlatformError::new(
                ErrorCode::RuntimeInvalid,
                "archive URL must be the official GitHub release host",
            ));
        }
        if url.query().is_some() || url.fragment().is_some() {
            return Err(PlatformError::new(
                ErrorCode::RuntimeInvalid,
                "archive URL must not contain a query or fragment",
            ));
        }
        let expected_path = format!(
            "/cloudflare/workerd/releases/download/{release}/{}",
            self.archive_name
        );
        if url.path() != expected_path {
            return Err(PlatformError::new(
                ErrorCode::RuntimeInvalid,
                "archive URL path must match the release and archive name",
            ));
        }
        let expected_archive = match target_name {
            "darwin-arm64" => "workerd-darwin-arm64.gz",
            "darwin-x64" => "workerd-darwin-64.gz",
            "linux-x64" => "workerd-linux-64.gz",
            "linux-arm64" => "workerd-linux-arm64.gz",
            _ => "",
        };
        if self.archive_name != expected_archive {
            return Err(PlatformError::new(
                ErrorCode::RuntimeInvalid,
                "archive name must match the official workerd naming",
            ));
        }
        parse_sha256_hex(&self.archive_sha256)?;
        parse_sha256_hex(&self.binary_sha256)?;
        Ok(())
    }
}

impl WorkersTypesPin {
    fn validate(&self, revision: &str) -> Result<(), PlatformError> {
        require_nonempty(&self.version, "workersTypes.version")?;
        require_git_sha(&self.git_head)?;
        if self.git_head != revision {
            return Err(PlatformError::new(
                ErrorCode::RuntimeInvalid,
                "workers-types gitHead must match the workerd revision",
            ));
        }
        parse_sha256_hex(&self.package_sha256)?;
        parse_sha256_hex(&self.ast_sha256)?;
        Ok(())
    }
}

impl WorkersSdkPin {
    fn validate(&self) -> Result<(), PlatformError> {
        require_git_sha(&self.revision)?;
        require_nonempty(&self.wrangler_version, "workersSdk.wranglerVersion")?;
        require_nonempty(&self.vite_plugin_version, "workersSdk.vitePluginVersion")?;
        Ok(())
    }
}

fn require_git_sha(value: &str) -> Result<(), PlatformError> {
    if value.len() != 40
        || !value
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
    {
        return Err(PlatformError::new(
            ErrorCode::RuntimeInvalid,
            "git revision must be a lowercase 40-character SHA-1",
        ));
    }
    Ok(())
}

fn require_compat_flags(flags: &[String]) -> Result<BTreeSet<&String>, PlatformError> {
    let mut seen = BTreeSet::new();
    for flag in flags {
        if flag.is_empty() || !flag.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
            return Err(PlatformError::new(
                ErrorCode::RuntimeInvalid,
                "compatibility flag is malformed",
            ));
        }
        if !seen.insert(flag) {
            return Err(PlatformError::new(
                ErrorCode::RuntimeInvalid,
                "compatibility flag is duplicated",
            ));
        }
    }
    Ok(seen)
}

fn require_nonempty(value: &str, _field: &str) -> Result<(), PlatformError> {
    if value.is_empty() || value.chars().any(char::is_whitespace) && value.trim() != value {
        return Err(PlatformError::new(
            ErrorCode::RuntimeInvalid,
            "lock field is empty or padded",
        ));
    }
    Ok(())
}

fn require_compat_date(value: &str) -> Result<(), PlatformError> {
    let bytes = value.as_bytes();
    if bytes.len() != 10
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || !bytes.iter().enumerate().all(|(i, b)| {
            if i == 4 || i == 7 {
                true
            } else {
                b.is_ascii_digit()
            }
        })
    {
        return Err(PlatformError::new(
            ErrorCode::RuntimeInvalid,
            "compatibility date must be YYYY-MM-DD",
        ));
    }
    let year = i32::from(bytes[0] - b'0') * 1_000
        + i32::from(bytes[1] - b'0') * 100
        + i32::from(bytes[2] - b'0') * 10
        + i32::from(bytes[3] - b'0');
    let month = u32::from(bytes[5] - b'0') * 10 + u32::from(bytes[6] - b'0');
    let day = u32::from(bytes[8] - b'0') * 10 + u32::from(bytes[9] - b'0');
    if !valid_gregorian_date(year, month, day) {
        return Err(PlatformError::new(
            ErrorCode::RuntimeInvalid,
            "compatibility date is not a real calendar date",
        ));
    }
    Ok(())
}

fn valid_gregorian_date(year: i32, month: u32, day: u32) -> bool {
    if year < 1970 || month == 0 || month > 12 || day == 0 {
        return false;
    }
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let dim = [
        0,
        31,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    day <= dim[month as usize]
}

fn validate_target_name(name: &str) -> Result<(), PlatformError> {
    match name {
        "darwin-arm64" | "darwin-x64" | "linux-x64" | "linux-arm64" => Ok(()),
        _ => Err(PlatformError::new(
            ErrorCode::RuntimeInvalid,
            "unknown workerd lock target name",
        )),
    }
}

fn current_target_name() -> Result<&'static str, PlatformError> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => Ok("darwin-arm64"),
        ("macos", "x86_64") => Ok("darwin-x64"),
        ("linux", "x86_64") => Ok("linux-x64"),
        ("linux", "aarch64") => Ok("linux-arm64"),
        _ => Err(PlatformError::new(
            ErrorCode::RuntimeInvalid,
            "current OS/arch is not a supported workerd target",
        )),
    }
}

/// Load a lock file from an absolute regular path without following symlinks.
pub fn load_runtime_lock(path: &Path) -> Result<(RuntimeLock, Vec<u8>), PlatformError> {
    require_absolute(path)?;
    require_regular_file(path)?;
    let bytes = read_regular_nofollow_bounded(path, MAX_LOCK_BYTES)?;
    Ok((RuntimeLock::parse(&bytes)?, bytes))
}

struct NoDupValue(serde_json::Value);

impl<'de> Deserialize<'de> for NoDupValue {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(NoDupVisitor)
    }
}

struct NoDupVisitor;

impl<'de> Visitor<'de> for NoDupVisitor {
    type Value = NoDupValue;

    fn expecting(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "a JSON value without duplicate object keys")
    }

    fn visit_bool<E: de::Error>(self, v: bool) -> Result<Self::Value, E> {
        Ok(NoDupValue(serde_json::Value::Bool(v)))
    }

    fn visit_i64<E: de::Error>(self, v: i64) -> Result<Self::Value, E> {
        Ok(NoDupValue(serde_json::Value::Number(v.into())))
    }

    fn visit_u64<E: de::Error>(self, v: u64) -> Result<Self::Value, E> {
        Ok(NoDupValue(serde_json::Value::Number(v.into())))
    }

    fn visit_f64<E: de::Error>(self, v: f64) -> Result<Self::Value, E> {
        let n = serde_json::Number::from_f64(v)
            .ok_or_else(|| de::Error::custom("invalid JSON number"))?;
        Ok(NoDupValue(serde_json::Value::Number(n)))
    }

    fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
        Ok(NoDupValue(serde_json::Value::String(v.to_owned())))
    }

    fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
        Ok(NoDupValue(serde_json::Value::Null))
    }

    fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
        Ok(NoDupValue(serde_json::Value::Null))
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
        let mut out = Vec::new();
        while let Some(NoDupValue(v)) = seq.next_element()? {
            out.push(v);
        }
        Ok(NoDupValue(serde_json::Value::Array(out)))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let mut out = serde_json::Map::new();
        while let Some((key, NoDupValue(value))) = map.next_entry::<String, NoDupValue>()? {
            if out.contains_key(&key) {
                return Err(de::Error::custom("duplicate JSON object key"));
            }
            out.insert(key, value);
        }
        Ok(NoDupValue(serde_json::Value::Object(out)))
    }
}

#[cfg(test)]
#[path = "lock_tests.rs"]
mod tests;
