//! Typed content-addressed artifact references.

use open_compute_core::{ErrorCode, PlatformError};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt::{Display, Formatter};
use std::str::FromStr;

/// Physical key version embedded in object keys.
pub const ARTIFACT_KEY_VERSION: u32 = 1;

/// Immutable content-addressed artifact identity.
///
/// Never contains endpoint, bucket, credential, or tenant-selected keys.
#[derive(Clone, Eq, PartialEq, Hash)]
pub struct ArtifactRef {
    version: u32,
    sha256: [u8; 32],
    size: u64,
}

impl ArtifactRef {
    /// Validate version, digest, and size.
    pub fn new(version: u32, sha256_hex: &str, size: u64) -> Result<Self, PlatformError> {
        if version != ARTIFACT_KEY_VERSION {
            return Err(PlatformError::new(
                ErrorCode::ConfigInvalid,
                "artifact version must be 1",
            ));
        }
        let sha256 = parse_sha256(sha256_hex)?;
        Ok(Self {
            version,
            sha256,
            size,
        })
    }

    /// Key version.
    #[must_use]
    pub const fn version(&self) -> u32 {
        self.version
    }

    /// Lowercase 64-hex SHA-256 digest.
    #[must_use]
    pub fn sha256_hex(&self) -> String {
        hex::encode(self.sha256)
    }

    /// Raw digest bytes.
    #[must_use]
    pub const fn sha256_bytes(&self) -> &[u8; 32] {
        &self.sha256
    }

    /// Declared size in bytes.
    #[must_use]
    pub const fn size(&self) -> u64 {
        self.size
    }

    /// Internal physical object key for `prefix`.
    #[must_use]
    pub fn physical_key(&self, prefix: &str) -> String {
        physical_key(prefix, &self.sha256_hex())
    }
}

impl Display for ArtifactRef {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "v{}/sha256/{}#{}",
            self.version,
            self.sha256_hex(),
            self.size
        )
    }
}

impl std::fmt::Debug for ArtifactRef {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ArtifactRef")
            .field("version", &self.version)
            .field("sha256", &self.sha256_hex())
            .field("size", &self.size)
            .finish()
    }
}

impl Serialize for ArtifactRef {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        #[derive(Serialize)]
        struct Wire<'a> {
            version: u32,
            sha256: &'a str,
            size: u64,
        }
        let hex = self.sha256_hex();
        Wire {
            version: self.version,
            sha256: &hex,
            size: self.size,
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for ArtifactRef {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Wire {
            version: u32,
            sha256: String,
            size: u64,
        }
        let wire = Wire::deserialize(deserializer)?;
        Self::new(wire.version, &wire.sha256, wire.size).map_err(serde::de::Error::custom)
    }
}

impl FromStr for ArtifactRef {
    type Err = PlatformError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let json = serde_json::from_str::<ArtifactRef>(s).map_err(|_| {
            PlatformError::new(ErrorCode::ConfigInvalid, "artifact ref json is invalid")
        })?;
        Ok(json)
    }
}

pub(crate) fn parse_sha256(hex_digest: &str) -> Result<[u8; 32], PlatformError> {
    if hex_digest.len() != 64 || !hex_digest.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(PlatformError::new(
            ErrorCode::ConfigInvalid,
            "artifact sha256 must be 64 lowercase hex characters",
        ));
    }
    if hex_digest.bytes().any(|b| b.is_ascii_uppercase()) {
        return Err(PlatformError::new(
            ErrorCode::ConfigInvalid,
            "artifact sha256 must be lowercase",
        ));
    }
    let mut out = [0_u8; 32];
    hex::decode_to_slice(hex_digest, &mut out).map_err(|_| {
        PlatformError::new(
            ErrorCode::ConfigInvalid,
            "artifact sha256 must be 64 lowercase hex characters",
        )
    })?;
    Ok(out)
}

pub(crate) fn physical_key(prefix: &str, sha256_hex: &str) -> String {
    format!(
        "{prefix}artifacts/v1/sha256/{}/{sha256_hex_rest}",
        &sha256_hex[..2],
        sha256_hex_rest = &sha256_hex[2..]
    )
}

pub(crate) fn parse_physical_key(prefix: &str, key: &str) -> Result<String, PlatformError> {
    let expected_prefix = format!("{prefix}artifacts/v1/sha256/");
    let rest = key.strip_prefix(&expected_prefix).ok_or_else(|| {
        PlatformError::new(
            ErrorCode::ConfigInvalid,
            "object key is not an internal artifact key",
        )
    })?;
    let (first, remaining) = rest.split_once('/').ok_or_else(|| {
        PlatformError::new(
            ErrorCode::ConfigInvalid,
            "object key is not an internal artifact key",
        )
    })?;
    if first.len() != 2 || remaining.len() != 62 {
        return Err(PlatformError::new(
            ErrorCode::ConfigInvalid,
            "object key is not an internal artifact key",
        ));
    }
    let digest = format!("{first}{remaining}");
    parse_sha256(&digest)?;
    Ok(digest)
}

#[cfg(test)]
#[path = "artifact_tests.rs"]
mod tests;
