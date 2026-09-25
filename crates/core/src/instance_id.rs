//! Stable instance identity persisted by the instance storage authority.

use crate::error::{ErrorCode, PlatformError};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt::{Display, Formatter};
use std::str::FromStr;
use uuid::Uuid;

/// Canonical instance identifier length (lowercase UUID bytes without separators).
pub const INSTANCE_ID_LEN: usize = 32;

/// One immutable instance identity.
///
/// The value is generated once when instance storage is initialized and is
/// independent of the configuration path, data path, or directory name.
#[derive(Clone, Copy, Eq, PartialEq, Hash)]
pub struct InstanceId {
    value: [u8; INSTANCE_ID_LEN],
    uuid: Uuid,
}

impl InstanceId {
    /// Generate a new `UUIDv7` instance identity.
    #[must_use]
    pub fn generate() -> Self {
        Self::from_uuid_unchecked(Uuid::now_v7())
    }

    /// Inner UUID.
    #[must_use]
    pub const fn as_uuid(&self) -> Uuid {
        self.uuid
    }

    /// Canonical 32-character lowercase hexadecimal form.
    #[must_use]
    #[allow(clippy::expect_used, reason = "UUID hex encoding is always ASCII")]
    pub fn as_str(&self) -> &str {
        std::str::from_utf8(&self.value).expect("UUID hex encoding is always ASCII")
    }

    /// Validate a `UUIDv7` and format it as the canonical instance identity.
    pub fn from_uuid(uuid: Uuid) -> Result<Self, PlatformError> {
        if uuid.get_version() != Some(uuid::Version::SortRand) {
            return Err(invalid_instance_id());
        }
        Ok(Self::from_uuid_unchecked(uuid))
    }

    fn from_uuid_unchecked(uuid: Uuid) -> Self {
        let mut value = [0; INSTANCE_ID_LEN];
        value.copy_from_slice(
            uuid.as_simple()
                .encode_lower(&mut Uuid::encode_buffer())
                .as_bytes(),
        );
        Self { value, uuid }
    }
}

impl Display for InstanceId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::fmt::Debug for InstanceId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("InstanceId").field(&self.as_str()).finish()
    }
}

impl FromStr for InstanceId {
    type Err = PlatformError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value.len() != INSTANCE_ID_LEN
            || value
                .bytes()
                .any(|byte| !byte.is_ascii_hexdigit() || byte.is_ascii_uppercase())
        {
            return Err(invalid_instance_id());
        }
        let uuid = Uuid::parse_str(value).map_err(|_| invalid_instance_id())?;
        let id = Self::from_uuid(uuid)?;
        if id.as_str() != value {
            return Err(invalid_instance_id());
        }
        Ok(id)
    }
}

impl Serialize for InstanceId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for InstanceId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

/// Optional operator-facing display and CLI selection name for an instance.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct InstanceName(String);

impl InstanceName {
    /// Validated name text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for InstanceName {
    type Err = PlatformError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let mut bytes = value.bytes();
        let valid = value.len() <= 32
            && bytes.next().is_some_and(|byte| byte.is_ascii_lowercase())
            && bytes.all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
            && !(value.len() == INSTANCE_ID_LEN
                && value.bytes().all(|byte| byte.is_ascii_hexdigit()));
        if !valid {
            return Err(PlatformError::new(
                ErrorCode::ConfigInvalid,
                "instance name must match [a-z][a-z0-9-]{0,31} and not resemble an instance ID",
            ));
        }
        Ok(Self(value.to_owned()))
    }
}

impl Display for InstanceName {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl Serialize for InstanceName {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for InstanceName {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

/// Instance ID or validated display name supplied by the operator CLI.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct InstanceSelector(String);

impl InstanceSelector {
    /// Validated selector text.
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
        Self(id.as_str().to_owned())
    }
}

impl FromStr for InstanceSelector {
    type Err = PlatformError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value.len() == INSTANCE_ID_LEN && value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            let id: InstanceId = value.parse()?;
            Ok(Self(id.as_str().to_owned()))
        } else {
            let name: InstanceName = value.parse()?;
            Ok(Self(name.0))
        }
    }
}

impl Serialize for InstanceSelector {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for InstanceSelector {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

fn invalid_instance_id() -> PlatformError {
    PlatformError::new(
        ErrorCode::InstanceIdInvalid,
        "instance ID must be a 32-character lowercase UUIDv7 hexadecimal value",
    )
}

#[cfg(test)]
#[path = "instance_id_tests.rs"]
mod tests;
