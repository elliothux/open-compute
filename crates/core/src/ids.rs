//! Canonical lowercase UUID identifiers; system RPC operations also accept `UUIDv4`.

use crate::error::{ErrorCode, PlatformError};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt::{Display, Formatter};
use std::str::FromStr;
use uuid::Uuid;

macro_rules! typed_id {
    ($name:ident, $label:literal) => {
        typed_id!($name, $label, "UUIDv7", validate_uuidv7);
    };
    ($name:ident, $label:literal, $versions:literal, $validate:ident) => {
        #[doc = concat!("Canonical lowercase ", $versions, " ", $label, " identifier.")]
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

            /// Validate and wrap a UUID of the version accepted by this identifier.
            pub fn from_uuid(uuid: Uuid) -> Result<Self, PlatformError> {
                $validate(uuid)?;
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
                let uuid = parse_canonical_uuid(s)?;
                Self::from_uuid(uuid)
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
typed_id!(DeploymentUploadId, "deployment upload session");
typed_id!(ResourceId, "resource");
typed_id!(BindingId, "deployment binding");
typed_id!(QueueId, "Queue resource");
typed_id!(QueueMessageId, "Queue message");
typed_id!(QueueConsumerId, "Queue consumer");
typed_id!(QueueBatchId, "Queue delivery batch");
typed_id!(CronActivationId, "Cron activation");
typed_id!(WorkflowId, "Workflow definition");
typed_id!(WorkflowVersionId, "Workflow version");
typed_id!(WorkflowInstanceId, "Workflow instance");
typed_id!(
    WorkflowOperationId,
    "Workflow restart or purge operation",
    "UUIDv4/UUIDv7",
    validate_operation_uuid
);
typed_id!(CronRunId, "Cron logical run");

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

fn validate_operation_uuid(uuid: Uuid) -> Result<(), PlatformError> {
    if !matches!(
        uuid.get_version(),
        Some(uuid::Version::Random | uuid::Version::SortRand)
    ) {
        return Err(PlatformError::new(
            ErrorCode::ConfigInvalid,
            "expected canonical UUIDv4 or UUIDv7 operation",
        ));
    }
    Ok(())
}

fn parse_canonical_uuid(s: &str) -> Result<Uuid, PlatformError> {
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
    Ok(uuid)
}

#[cfg(test)]
#[path = "ids_tests.rs"]
mod tests;
