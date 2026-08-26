//! Foundation types for the Open Compute platform.
//!
//! This crate is dependency-light and contains no storage, S3, runtime, or CLI
//! behavior. Later crates consume these contracts without resolving secrets at
//! parse time.

#![deny(missing_docs)]

pub mod admission;
pub mod capability;
pub mod clock;
pub mod config;
pub mod durable_objects;
pub mod error;
pub mod health;
pub mod ids;
pub mod redact;
pub mod release_identity;
pub mod resource;
pub mod scheduler;
pub mod secret;
pub mod snapshot_manifest;

pub use admission::{
    AdmissionReservation, AdmissionReservations, AdmissionSnapshotV1, OperationClass, PlatformMode,
};
pub use capability::{
    CapabilityStatus, PlatformCapabilitiesV1, ProductCapabilityV1, RuntimeCapabilityV1,
};
pub use clock::{Clock, SystemClock};
pub use config::{
    CacheConfig, D1Config, DiagnosticsConfig, DurableObjectsConfig, HardeningConfig, KvConfig,
    MetricsConfig, PlatformConfig, R2Config, RuntimeConfig, S3Config, SchedulerConfig,
    SecretReference, ServerConfig, StorageConfig, WorkersConfig, validate_bootstrap_config_path,
};
pub use durable_objects::{
    DURABLE_OBJECT_ID_BYTES, DURABLE_OBJECT_NAME_MAX_BYTES, DURABLE_OBJECT_NAMESPACE_PREFIX_BYTES,
    DurableObjectId, DurableObjectState, durable_object_namespace_prefix,
};
pub use error::{ErrorCode, PlatformError, ReadinessReason};
pub use health::{ComponentHealth, ComponentName, ComponentState, PlatformStatus};
pub use ids::{
    AccountId, BindingId, DeploymentId, PlatformId, RequestId, ResourceId, StartupId, WorkerId,
};
pub use redact::Redactor;
pub use release_identity::{
    PlatformReleaseIdentityV1, PlatformReleaseMetadataV1, ReleaseMigrationV1,
};
pub use resource::{
    BindingKind, CanonicalBindingConfig, CanonicalPermissions, ResourceAvailability, ResourceState,
};
#[cfg(any(test, feature = "test-support"))]
pub use scheduler::DeterministicSchedulerClock;
pub use scheduler::{SchedulerClock, SystemSchedulerClock};
pub use secret::{SecretBytes, SecretString};
pub use snapshot_manifest::{
    PlatformSnapshotManifestV1, SnapshotFileRole, SnapshotFileV1, SnapshotImmutableReferenceV1,
    SnapshotTotalsV1, valid_restore_path,
};

#[cfg(any(test, feature = "test-support"))]
pub use clock::DeterministicClock;
