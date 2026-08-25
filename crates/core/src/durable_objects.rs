//! Durable Object public identity and lifecycle value types.

use crate::{ErrorCode, PlatformError, ResourceId};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fmt::{Display, Formatter};
use std::str::FromStr;

/// Size of a public Durable Object identity in bytes.
pub const DURABLE_OBJECT_ID_BYTES: usize = 32;
/// Size of the namespace discriminator at the start of a public identity.
pub const DURABLE_OBJECT_NAMESPACE_PREFIX_BYTES: usize = 8;
/// Maximum UTF-8 byte length accepted by `idFromName()`.
pub const DURABLE_OBJECT_NAME_MAX_BYTES: usize = 1024;

/// Canonical 32-byte public Durable Object identity.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct DurableObjectId([u8; DURABLE_OBJECT_ID_BYTES]);

impl DurableObjectId {
    /// Construct an identity after checking its namespace prefix.
    pub fn for_namespace(
        bytes: [u8; DURABLE_OBJECT_ID_BYTES],
        namespace_id: ResourceId,
    ) -> Result<Self, PlatformError> {
        let value = Self(bytes);
        if !value.belongs_to(namespace_id) {
            return Err(invalid_id());
        }
        Ok(value)
    }

    /// Return the raw canonical bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; DURABLE_OBJECT_ID_BYTES] {
        &self.0
    }

    /// Test whether this identity belongs to a namespace.
    #[must_use]
    pub fn belongs_to(self, namespace_id: ResourceId) -> bool {
        self.0[..DURABLE_OBJECT_NAMESPACE_PREFIX_BYTES]
            == durable_object_namespace_prefix(namespace_id)
    }
}

impl Display for DurableObjectId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(&hex::encode(self.0))
    }
}

impl FromStr for DurableObjectId {
    type Err = PlatformError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value.len() != DURABLE_OBJECT_ID_BYTES * 2
            || value
                .bytes()
                .any(|byte| !matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
        {
            return Err(invalid_id());
        }
        let bytes = hex::decode(value).map_err(|_| invalid_id())?;
        let bytes = bytes.try_into().map_err(|_| invalid_id())?;
        Ok(Self(bytes))
    }
}

impl Serialize for DurableObjectId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for DurableObjectId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        value.parse().map_err(serde::de::Error::custom)
    }
}

/// Durable Object registry state used only for lifecycle fencing.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DurableObjectState {
    /// Identity is registered but native dispatch has not yet been acknowledged.
    Creating,
    /// Dispatch is admitted.
    Ready,
    /// New dispatch is fenced while native facet deletion converges.
    Deleting,
    /// This object generation is permanently retired.
    Tombstoned,
}

impl DurableObjectState {
    /// Stable database token.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Creating => "creating",
            Self::Ready => "ready",
            Self::Deleting => "deleting",
            Self::Tombstoned => "tombstoned",
        }
    }
}

impl FromStr for DurableObjectState {
    type Err = PlatformError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "creating" => Ok(Self::Creating),
            "ready" => Ok(Self::Ready),
            "deleting" => Ok(Self::Deleting),
            "tombstoned" => Ok(Self::Tombstoned),
            _ => Err(invariant()),
        }
    }
}

/// Derive the stable eight-byte public discriminator for a namespace.
#[must_use]
pub fn durable_object_namespace_prefix(namespace_id: ResourceId) -> [u8; 8] {
    let mut digest = Sha256::new();
    digest.update(b"oc-do-ns-v1");
    digest.update(namespace_id.to_string().as_bytes());
    let hash = digest.finalize();
    let mut prefix = [0_u8; 8];
    prefix.copy_from_slice(&hash[..8]);
    prefix
}

fn invalid_id() -> PlatformError {
    PlatformError::new(
        ErrorCode::DoIdInvalid,
        "Durable Object identity is invalid for this namespace",
    )
}

fn invariant() -> PlatformError {
    PlatformError::new(
        ErrorCode::ResourceInvariantViolation,
        "persisted Durable Object lifecycle is invalid",
    )
}

#[cfg(test)]
#[path = "durable_objects_tests.rs"]
mod tests;
