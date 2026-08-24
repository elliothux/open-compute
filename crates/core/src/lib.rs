//! Foundation types for the Open Compute platform.
//!
//! This crate is dependency-light and contains no storage, S3, runtime, or CLI
//! behavior. Later crates consume these contracts without resolving secrets at
//! parse time.

#![deny(missing_docs)]

pub mod clock;
pub mod config;
pub mod error;
pub mod health;
pub mod ids;
pub mod redact;
pub mod resource;
pub mod secret;

pub use clock::{Clock, SystemClock};
pub use config::{
    CacheConfig, DiagnosticsConfig, KvConfig, MetricsConfig, PlatformConfig, RuntimeConfig,
    S3Config, SecretReference, ServerConfig, StorageConfig, WorkersConfig,
    validate_bootstrap_config_path,
};
pub use error::{ErrorCode, PlatformError, ReadinessReason};
pub use health::{ComponentHealth, ComponentName, ComponentState, PlatformStatus};
pub use ids::{
    AccountId, BindingId, DeploymentId, PlatformId, RequestId, ResourceId, StartupId, WorkerId,
};
pub use redact::Redactor;
pub use resource::{
    BindingKind, CanonicalBindingConfig, CanonicalPermissions, ResourceAvailability, ResourceState,
};
pub use secret::{SecretBytes, SecretString};

#[cfg(any(test, feature = "test-support"))]
pub use clock::DeterministicClock;
