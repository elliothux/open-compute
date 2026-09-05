//! Foundation types for the Open Compute platform.
//!
//! This crate is dependency-light and contains no storage adapter, runtime, or CLI
//! behavior. Later crates consume these contracts without resolving secrets at
//! parse time.

#![deny(missing_docs)]

pub mod admission;
pub mod capability;
pub mod clock;
pub mod config;
pub mod cron;
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
pub mod workflow;

pub use admission::{
    AdmissionReservation, AdmissionReservations, AdmissionSnapshotV1, OperationClass, PlatformMode,
};
pub use capability::{
    CapabilityInventoryV1, CapabilityMemberV1, CapabilityStatus, InterfaceCapabilityStatus,
    LegacyManagementRouteV1, ManagementApiCapabilitiesV1, ManagementApiMethod,
    ManagementApiRequestMediaType, ManagementApiRouteV1, ObservabilityCapabilityItemV1,
    PlatformCapabilitiesV1, ProductCapabilityV1, ProductKind, RuntimeCapabilityV1,
    TypeSourceIdentityV1, WorkersObservabilityCapabilitiesV1, WranglerCapabilitiesV1,
    WranglerCapabilityItemV1,
};
pub use clock::{Clock, SystemClock};
pub use config::{
    AiAuthConfig, AiConfig, AiEmbeddingMetric, AiEmbeddingModelConfig, AiGenerationCapability,
    AiGenerationModelConfig, AiProviderConfig, AiTokenizer, AiTokenizerArtifactConfig, CacheConfig,
    D1Config, DataConfig, DocumentParserConfig, DurableObjectsConfig, HardeningConfig,
    ImagesConfig, KvConfig, LocalObjectStorageConfig, MetricsConfig, ObjectStorageConfig,
    ObjectStorageKind, PlatformConfig, QueuesConfig, R2Config, ResolvedEmbeddingModelContract,
    ResolvedTokenizerContract, ResponseCacheConfig, RuntimeConfig, S3Config, SchedulerConfig,
    SchedulerPoolConfig, SchedulerPoolsConfig, SecretReference, ServerConfig, WorkersConfig,
    validate_bootstrap_config_path,
};
pub use cron::CronSchedule;
pub use durable_objects::{
    DURABLE_OBJECT_ID_BYTES, DURABLE_OBJECT_NAME_MAX_BYTES, DURABLE_OBJECT_NAMESPACE_PREFIX_BYTES,
    DurableObjectId, DurableObjectState, durable_object_namespace_prefix,
};
pub use error::{ErrorCode, PlatformError, ReadinessReason};
pub use health::{ComponentHealth, ComponentName, ComponentState, PlatformStatus};
pub use ids::{
    AccountId, BindingId, CronActivationId, CronRunId, DeploymentId, PlatformId, QueueBatchId,
    QueueConsumerId, QueueId, QueueMessageId, RequestId, ResourceId, StartupId, VersionId,
    VersionUploadId, WorkerId, WorkflowId, WorkflowInstanceId, WorkflowOperationId,
    WorkflowVersionId,
};
pub use redact::Redactor;
pub use release_identity::{
    PlatformReleaseIdentityV1, PlatformReleaseMetadataV1, ReleaseSchemaDefinitionV1,
};
pub use resource::{
    BindingKind, CanonicalBindingConfig, CanonicalPermissions, ResourceAvailability, ResourceState,
};
#[cfg(any(test, feature = "test-support"))]
pub use scheduler::{DeterministicSchedulerClock, SchedulerFaultPoint};
pub use scheduler::{
    DispatchOutcome, SchedulerClock, SchedulerFenceV1, SchedulerKind, SchedulerPoolState,
    SchedulerSleep, SystemSchedulerClock, WorkloadSummary,
};
pub use secret::{SecretBytes, SecretString};
pub use snapshot_manifest::{
    PlatformSnapshotManifestV1, SnapshotFileRole, SnapshotFileV1, SnapshotImmutableReferenceV1,
    SnapshotTotalsV1, valid_restore_path,
};
pub use workflow::{WorkflowCronSchedule, WorkflowFence, WorkflowToken, WorkflowsConfig};

#[cfg(any(test, feature = "test-support"))]
pub use clock::DeterministicClock;
