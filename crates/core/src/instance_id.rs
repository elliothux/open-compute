//! Stable local instance identity derived from a canonical config path.

use crate::error::{ErrorCode, PlatformError};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sha2::{Digest, Sha256};
use std::fmt::{Display, Formatter};
use std::path::Path;
use std::str::FromStr;

/// Domain separator for config-path instance digests.
const DOMAIN: &[u8] = b"open-compute/instance-id/v1\0";

/// Crockford base32 alphabet without `i`, `l`, `o`, or `u`.
const CROCKFORD: &[u8; 32] = b"0123456789abcdefghjkmnpqrstvwxyz";

/// Minimum short-ID length written to the instance registry.
pub const INSTANCE_ID_MIN_LEN: usize = 5;

/// Maximum short-ID length used when extending past collisions.
pub const INSTANCE_ID_MAX_LEN: usize = 52;

/// Local operator instance identity derived only from a canonical config path.
///
/// The short ID is a lowercase Crockford base32 prefix of the digest. Registry
/// writes compare the full digest and may extend the prefix on collision; an
/// existing instance never changes ID.
#[derive(Clone, Eq, PartialEq, Hash)]
pub struct InstanceId {
    short: String,
    digest: [u8; 32],
}

impl InstanceId {
    /// Derive the default five-character candidate from a canonical absolute config path.
    pub fn from_canonical_config_path(path: &Path) -> Result<Self, PlatformError> {
        let digest = digest_canonical_config_path(path)?;
        Self::from_digest(digest, INSTANCE_ID_MIN_LEN)
    }

    /// Build an ID from a digest using exactly `len` Crockford characters.
    pub fn from_digest(digest: [u8; 32], len: usize) -> Result<Self, PlatformError> {
        if !(INSTANCE_ID_MIN_LEN..=INSTANCE_ID_MAX_LEN).contains(&len) {
            return Err(PlatformError::new(
                ErrorCode::InstanceIdInvalid,
                "instance ID length is outside the supported range",
            ));
        }
        let encoded = encode_crockford(&digest);
        let short = encoded[..len].to_string();
        Ok(Self { short, digest })
    }

    /// Reconstruct an ID from a persisted short prefix and the path digest.
    pub fn from_short_and_digest(short: &str, digest: [u8; 32]) -> Result<Self, PlatformError> {
        parse_short_id(short)?;
        let expected = Self::from_digest(digest, short.len())?;
        if expected.short != short {
            return Err(PlatformError::new(
                ErrorCode::InstanceIdInvalid,
                "instance ID does not match its config-path digest",
            ));
        }
        Ok(expected)
    }

    /// Full SHA-256 digest used for registry collision checks.
    #[must_use]
    pub fn digest(&self) -> &[u8; 32] {
        &self.digest
    }

    /// Persisted short ID string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.short
    }

    /// Character length of the persisted short ID.
    #[must_use]
    pub fn len(&self) -> usize {
        self.short.len()
    }

    /// Short IDs are never empty once constructed.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.short.is_empty()
    }

    /// Return the same digest encoded with one additional Crockford character.
    pub fn extend_one(&self) -> Result<Self, PlatformError> {
        Self::from_digest(self.digest, self.short.len().saturating_add(1))
    }
}

impl Display for InstanceId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.short)
    }
}

impl std::fmt::Debug for InstanceId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("InstanceId").field(&self.short).finish()
    }
}

impl Serialize for InstanceId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.short)
    }
}

/// Exact short-ID selector supplied on the CLI (`--instance`).
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct InstanceSelector(String);

impl InstanceSelector {
    /// Validated short ID text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Display for InstanceSelector {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<InstanceId> for InstanceSelector {
    fn from(id: InstanceId) -> Self {
        Self(id.short)
    }
}

impl FromStr for InstanceSelector {
    type Err = PlatformError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        parse_short_id(s)?;
        Ok(Self(s.to_owned()))
    }
}

impl Serialize for InstanceSelector {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for InstanceSelector {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        value.parse().map_err(serde::de::Error::custom)
    }
}

/// Compute the instance digest for a canonical absolute config path.
pub fn digest_canonical_config_path(path: &Path) -> Result<[u8; 32], PlatformError> {
    if !path.is_absolute() {
        return Err(PlatformError::new(
            ErrorCode::ConfigPathInvalid,
            "instance ID requires a canonical absolute config path",
        ));
    }
    let bytes = path_bytes(path)?;
    let mut hasher = Sha256::new();
    hasher.update(DOMAIN);
    hasher.update(bytes);
    Ok(hasher.finalize().into())
}

/// Validate a persisted short instance ID without recovering its digest.
pub fn parse_short_id(value: &str) -> Result<(), PlatformError> {
    if !(INSTANCE_ID_MIN_LEN..=INSTANCE_ID_MAX_LEN).contains(&value.len()) {
        return Err(PlatformError::new(
            ErrorCode::InstanceIdInvalid,
            "instance ID length is outside the supported range",
        ));
    }
    if value.bytes().any(|b| !CROCKFORD.contains(&b)) {
        return Err(PlatformError::new(
            ErrorCode::InstanceIdInvalid,
            "instance ID must be lowercase Crockford base32",
        ));
    }
    Ok(())
}

fn path_bytes(path: &Path) -> Result<&[u8], PlatformError> {
    use std::os::unix::ffi::OsStrExt;
    let bytes = path.as_os_str().as_bytes();
    if bytes.is_empty() {
        return Err(PlatformError::new(
            ErrorCode::ConfigPathInvalid,
            "instance ID config path is empty",
        ));
    }
    Ok(bytes)
}

fn encode_crockford(digest: &[u8; 32]) -> String {
    // 256 bits → 52 Crockford characters (260 bits with 4 zero pad bits).
    let mut out = String::with_capacity(INSTANCE_ID_MAX_LEN);
    let mut buffer: u64 = 0;
    let mut bits: u32 = 0;
    for &byte in digest {
        buffer = (buffer << 8) | u64::from(byte);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            let index = ((buffer >> bits) & 0x1f) as usize;
            out.push(CROCKFORD[index] as char);
        }
    }
    if bits > 0 {
        let index = ((buffer << (5 - bits)) & 0x1f) as usize;
        out.push(CROCKFORD[index] as char);
    }
    debug_assert_eq!(out.len(), INSTANCE_ID_MAX_LEN);
    out
}

#[cfg(test)]
#[path = "instance_id_tests.rs"]
mod tests;
