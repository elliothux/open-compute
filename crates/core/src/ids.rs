//! Canonical lowercase UUID version 7 identifiers.

use crate::error::{ErrorCode, PlatformError};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt::{Display, Formatter};
use std::str::FromStr;
use uuid::Uuid;

macro_rules! typed_id {
    ($name:ident, $label:literal) => {
        #[doc = concat!("Canonical lowercase UUIDv7 ", $label, " identifier.")]
        #[derive(Clone, Copy, Eq, PartialEq, Hash)]
        pub struct $name(Uuid);

        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.serialize_str(&canonical(self.0))
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                let value = String::deserialize(deserializer)?;
                value.parse().map_err(serde::de::Error::custom)
            }
        }

        impl $name {
            /// Generate a new UUID version 7 identifier.
            #[must_use]
            pub fn generate() -> Self {
                Self(Uuid::now_v7())
            }

            /// Wrap an already-validated UUID version 7.
            pub fn from_uuid(uuid: Uuid) -> Result<Self, PlatformError> {
                validate_uuidv7(uuid)?;
                Ok(Self(uuid))
            }

            /// Inner UUID.
            #[must_use]
            pub const fn as_uuid(self) -> Uuid {
                self.0
            }

            /// Canonical lowercase hyphenated form.
            #[must_use]
            pub fn as_canonical_str(&self) -> String {
                canonical(self.0)
            }
        }

        impl Display for $name {
            fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
                f.write_str(&canonical(self.0))
            }
        }

        impl std::fmt::Debug for $name {
            fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
                f.debug_tuple(stringify!($name))
                    .field(&canonical(self.0))
                    .finish()
            }
        }

        impl FromStr for $name {
            type Err = PlatformError;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                parse_canonical_uuidv7(s).map(Self)
            }
        }
    };
}

typed_id!(PlatformId, "platform instance");
typed_id!(AccountId, "account");
typed_id!(StartupId, "startup generation");
typed_id!(RequestId, "request");
typed_id!(WorkerId, "worker");
typed_id!(DeploymentId, "deployment");
typed_id!(ResourceId, "resource");
typed_id!(BindingId, "deployment binding");

fn canonical(uuid: Uuid) -> String {
    uuid.as_hyphenated()
        .encode_lower(&mut Uuid::encode_buffer())
        .to_string()
}

fn validate_uuidv7(uuid: Uuid) -> Result<(), PlatformError> {
    if uuid.get_version() != Some(uuid::Version::SortRand) {
        return Err(PlatformError::new(
            ErrorCode::ConfigInvalid,
            "expected canonical lowercase UUIDv7",
        ));
    }
    Ok(())
}

fn parse_canonical_uuidv7(s: &str) -> Result<Uuid, PlatformError> {
    if s.chars().any(|c| c.is_ascii_uppercase()) {
        return Err(PlatformError::new(
            ErrorCode::ConfigInvalid,
            "UUID must be canonical lowercase hyphenated form",
        ));
    }
    let uuid = Uuid::parse_str(s).map_err(|_| {
        PlatformError::new(
            ErrorCode::ConfigInvalid,
            "UUID is not a valid hyphenated UUID",
        )
    })?;
    if canonical(uuid) != s {
        return Err(PlatformError::new(
            ErrorCode::ConfigInvalid,
            "UUID must be canonical 8-4-4-4-12 lowercase form",
        ));
    }
    validate_uuidv7(uuid)?;
    Ok(uuid)
}

#[cfg(test)]
#[path = "ids_tests.rs"]
mod tests;
