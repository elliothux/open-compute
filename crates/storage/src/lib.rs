//! Secure data-directory ownership, control database, identity, and secret crypto.

#![deny(missing_docs)]

pub mod assets;
pub mod bindings;
pub mod control_db;
pub mod cron;
pub mod crypto;
pub mod d1;
pub mod data_dir;
pub mod disk_admission;
pub mod durable_objects;
pub mod fs;
pub mod identity;
pub mod inspect;
pub mod kv;
pub mod lock;
pub mod master_key;
pub mod migrations;
pub mod platform_restore;
pub mod platform_snapshot;
pub mod queue_consumers;
pub mod queues;
pub mod r2;
pub mod r2_staging;
pub mod resources;
mod restore_cleanup;
pub mod scheduler;
mod schema_inspection;
pub mod services;
mod snapshot_staging;
pub mod workers;
pub mod workflows;
pub use workflows::{
    WorkflowAppliedOperation, WorkflowBindingDescriptor, WorkflowBindingRecord, WorkflowDefinition,
    WorkflowGcAcknowledgement, WorkflowGcReceipt, WorkflowInstanceIdentity, WorkflowOperation,
    WorkflowOperationInspection, WorkflowOperationKind, WorkflowOperationResult, WorkflowRefState,
    WorkflowRejectedOperation, WorkflowRepository, WorkflowReservation, WorkflowTarget,
    WorkflowVersion,
};

pub use assets::{
    BeginDeploymentUploadFinalize, DeploymentAssetsRecord, DeploymentAssetsRepository,
    DeploymentObjectKind, DeploymentUploadFinalize, DeploymentUploadFinalizeDisposition,
    DeploymentUploadObjectRecord, DeploymentUploadRecord, DeploymentUploadRepository,
    DeploymentUploadStatus, NewDeploymentAssets, NewDeploymentObjectRef, NewDeploymentUpload,
    NewDeploymentUploadObject,
};
pub use bindings::{
    AuthorizedBinding, BindingRepository, DeploymentBindingRecord, NewDeploymentBinding,
};
pub use control_db::ControlDb;
pub use cron::{
    CRON_PARSER_VERSION, CronActivationRecord, CronActivationState, CronDeclaration,
    CronDeclarationMode, CronDeploymentConfig, CronRepository, NewCronConfig, NewCronDeclaration,
};
pub use crypto::{SecretCrypto, SecretEnvelope};
pub use d1::{
    D1_DATABASE_SCHEMA_VERSION, D1_MAX_BATCH_STATEMENTS, D1_MAX_BOUND_PARAMS, D1_MAX_COLUMNS,
    D1_MAX_EXEC_STATEMENTS, D1_MAX_SQL_BYTES, D1_MAX_VALUE_OR_ROW_BYTES, D1BackupRecord,
    D1BackupState, D1DatabaseRecord, D1DatabaseRepository, D1Engine, D1ExecResult, D1Meta,
    D1Migration, D1MigrationRecord, D1Paths, D1QueryLimits, D1Statement, D1StatementResult,
    D1Value,
};
pub use data_dir::{
    DURABLE_OBJECT_DATA_FORMAT_VERSION, DURABLE_OBJECT_UNIQUE_KEY, DataDir,
    inspect_durable_object_storage, read_operation_receipt,
};
#[cfg(any(test, feature = "test-support"))]
pub use data_dir::{expected_directories, future_resource_paths};
pub use disk_admission::DiskAdmission;
pub use durable_objects::{
    AuthorizedDurableObjectDelete, AuthorizedDurableObjectDispatch, DO_NAMESPACE_SCHEMA_VERSION,
    DurableObjectNamespaceRecord, DurableObjectRecord, DurableObjectRepository,
};
pub use fs::atomic_write;
pub use identity::{ARTIFACT_SCHEMA_VERSION, StableIdentity};
pub use inspect::{
    ControlInventory, DataRootInspect, ResourceInspect, inspect_control_db,
    inspect_control_inventory, inspect_data_root, inspect_master_key, inspect_operator_event_count,
    inspect_resources, inspect_snapshot_immutable_references,
};
pub use kv::{
    KV_CAPABILITY_VERSION, KV_DEFAULT_LIST_LIMIT, KV_MAX_KEY_BYTES, KV_MAX_LIST_LIMIT,
    KV_MAX_METADATA_BYTES, KV_MAX_MULTI_GET_KEYS, KV_MAX_MULTI_GET_RESPONSE_BYTES,
    KV_MAX_VALUE_BYTES, KV_MIN_CACHE_TTL_SECONDS, KV_MIN_EXPIRATION_TTL_SECONDS, KV_SCHEMA_VERSION,
    KvBackupRecord, KvBackupState, KvEngine, KvEntry, KvEntryInfo, KvListPage, KvListRow,
    KvNamespaceRecord, KvNamespaceRepository, KvPaths, KvPutOptions, canonical_metadata,
    validate_key,
};
pub use lock::{DataDirLock, FilesystemDurability, InspectLock};
pub use master_key::MasterKey;
#[cfg(any(test, feature = "test-support"))]
pub use master_key::{clear_test_env, set_test_env};
#[cfg(any(test, feature = "test-support"))]
pub use migrations::MigrationFault;
pub use platform_restore::RestoreTarget;
pub use platform_snapshot::{
    PreparePlatformSnapshotRequest, PreparedPlatformSnapshot, PreparedSnapshotFile,
    estimate_platform_snapshot_bytes, prepare_platform_snapshot, sign_snapshot_manifest,
    verify_snapshot_manifest_mac,
};
pub use queue_consumers::{
    NewQueueConsumerDeclaration, QUEUE_CONSUMER_DEFAULT_BATCH_SIZE,
    QUEUE_CONSUMER_DEFAULT_BATCH_TIMEOUT_SECONDS, QUEUE_CONSUMER_DEFAULT_MAX_CONCURRENCY,
    QUEUE_CONSUMER_DEFAULT_MAX_RETRIES, QUEUE_CONSUMER_DEFAULT_RETRY_DELAY_SECONDS,
    QueueConsumerConfig, QueueConsumerDeclaration, QueueConsumerRecord, QueueConsumerRepository,
    QueueConsumerState,
};
pub use queues::{
    AuthorizedQueueBinding, NewQueueProducerBinding, QUEUE_DEFAULT_MAX_BACKLOG_BYTES,
    QUEUE_DEFAULT_RETENTION_SECONDS, QUEUE_MAX_BATCH_BYTES, QUEUE_MAX_BATCH_MESSAGES,
    QUEUE_MAX_DELAY_SECONDS, QUEUE_MAX_MESSAGE_BYTES, QUEUE_MAX_RETENTION_SECONDS,
    QUEUE_MIN_RETENTION_SECONDS, QUEUE_PRODUCER_CAPABILITY_VERSION, QueueAvailability, QueueConfig,
    QueueCreateReservation, QueueProducerBindingRecord, QueueRecord, QueueRepository, QueueState,
    RunningQueueMutation,
};
pub use r2::{R2_SCHEMA_VERSION, R2BucketRecord, R2BucketRepository};
pub use r2_staging::R2Staging;
pub use resources::{
    ReserveResourceCreate, ReserveResourceDelete, ResourceCreateReservation,
    ResourceDeleteReservation, ResourceRecord, ResourceReferrer, ResourceRepository,
};
pub use restore_cleanup::{RestoreStagingCleanup, cleanup_restore_staging};
pub use scheduler::{
    AlarmProjection, ClaimResult, ClaimedCronRun, ClaimedJob, ClaimedQueueBatch,
    ClaimedQueueMessage, CronCompletion, CronCompletionResult, CronInspectionSummary,
    CronRuntimeInspection, CronScheduleProjection, CronSlotSummary, P23CrossDatabaseInspection,
    QueueCompletionAction, QueueCompletionDecision, QueueCompletionSummary,
    QueueConsumerInspectionSummary, QueueConsumerProjection, QueueConsumerRuntimeInspection,
    QueueContentType, QueueCounterMismatch, QueueDeleteBatch, QueueDlqForwardSummary,
    QueueEnqueueRequest, QueueEnqueueResult, QueueInspectionSummary, QueueMessageInput,
    QueueMetrics, QueueProjection, SchedulerInspection, SchedulerStore, SchedulerSummary,
    SchedulerWakeFuture, SchedulerWakeSignal, current_scheduler_schema_version,
    inspect_p23_cross_database, inspect_scheduler_db, scheduler_migration_registry,
};
pub use schema_inspection::{CurrentSchemaState, inspect_current_schema};
pub use services::{
    DeploymentServiceRecord, NewDeploymentService, ResolvedServiceTarget, ServiceReferrer,
    ServiceRepository,
};
pub use snapshot_staging::{LocalSnapshotStagingCleanup, cleanup_stale_snapshot_staging};
pub use workers::{
    DeploymentContentKind, DeploymentRecord, DeploymentReferrer, DeploymentSnapshot,
    DeploymentState, IdempotencyReservation, LOADER_SCHEMA_VERSION, NewDeployment,
    NewDeploymentProducts, RetentionCandidate, RouteKind, RouteRecord, RouteSnapshot,
    StoredDeploymentSecret, WorkerRecord, WorkerRepository,
};

use open_compute_core::clock::Clock;
use open_compute_core::config::StorageConfig;
use open_compute_core::{
    AdmissionReservation, AdmissionSnapshotV1, HardeningConfig, PlatformError,
};

/// Fully bootstrapped P0.1 storage owner.
#[derive(Debug)]
pub struct PlatformStorage {
    data_dir: DataDir,
    db: ControlDb,
    crypto: SecretCrypto,
    identity: StableIdentity,
    free_space_hard_bytes: u64,
    hardening: HardeningConfig,
    admission: DiskAdmission,
}

impl PlatformStorage {
    /// Acquire the data-dir lock, resolve the master key, then open/migrate the DB and identity.
    pub fn bootstrap(config: &StorageConfig, clock: &dyn Clock) -> Result<Self, PlatformError> {
        let mut hardening = HardeningConfig::default();
        hardening.emergency_reserve_bytes = hardening
            .emergency_reserve_bytes
            .min(config.free_space_hard_bytes.saturating_sub(1));
        Self::bootstrap_with_hardening(config, &hardening, clock)
    }

    /// Bootstrap with the P1 platform-wide hardening policy.
    pub fn bootstrap_with_hardening(
        config: &StorageConfig,
        hardening: &HardeningConfig,
        clock: &dyn Clock,
    ) -> Result<Self, PlatformError> {
        let data_dir = DataDir::acquire(config)?;
        let key = master_key::resolve(config)?;
        let db_path = data_dir.ensure_control_db()?;
        let db = ControlDb::open(&db_path, config.sqlite_busy_timeout_ms)?;
        db.migrate(clock)?;
        let identity = identity::bootstrap(&db, clock, key.fingerprint())?;
        data_dir.record_platform_id(&identity.platform_id.to_string())?;
        let crypto = SecretCrypto::new(key.bytes(), key.fingerprint())?;
        Ok(Self {
            data_dir,
            db,
            crypto,
            identity,
            free_space_hard_bytes: config.free_space_hard_bytes,
            hardening: hardening.clone(),
            admission: DiskAdmission::new(config, hardening),
        })
    }

    /// Bootstrap with optional test-only migration fault injection.
    #[cfg(any(test, feature = "test-support"))]
    pub fn bootstrap_with_fault(
        config: &StorageConfig,
        clock: &dyn Clock,
        fault: Option<MigrationFault>,
    ) -> Result<Self, PlatformError> {
        let data_dir = DataDir::acquire(config)?;
        let key = master_key::resolve(config)?;
        let db_path = data_dir.ensure_control_db()?;
        let db = ControlDb::open(&db_path, config.sqlite_busy_timeout_ms)?;
        db.migrate_with_fault(clock, fault)?;
        let identity = identity::bootstrap(&db, clock, key.fingerprint())?;
        data_dir.record_platform_id(&identity.platform_id.to_string())?;
        let crypto = SecretCrypto::new(key.bytes(), key.fingerprint())?;
        let mut hardening = HardeningConfig::default();
        hardening.emergency_reserve_bytes = hardening
            .emergency_reserve_bytes
            .min(config.free_space_hard_bytes.saturating_sub(1));
        Ok(Self {
            data_dir,
            db,
            crypto,
            identity,
            free_space_hard_bytes: config.free_space_hard_bytes,
            hardening: hardening.clone(),
            admission: DiskAdmission::new(config, &hardening),
        })
    }

    /// Data directory owner (holds the exclusive lock).
    #[must_use]
    pub fn data_dir(&self) -> &DataDir {
        &self.data_dir
    }

    /// Control database.
    #[must_use]
    pub fn db(&self) -> &ControlDb {
        &self.db
    }

    /// Secret crypto bound to the resolved master key.
    #[must_use]
    pub fn crypto(&self) -> &SecretCrypto {
        &self.crypto
    }

    /// Stable identity.
    #[must_use]
    pub fn identity(&self) -> &StableIdentity {
        &self.identity
    }

    /// Filesystem safety floor below which new durable bytes are refused.
    #[must_use]
    pub const fn free_space_hard_bytes(&self) -> u64 {
        self.free_space_hard_bytes
    }

    /// P1 resource-count, snapshot, and emergency-reserve policy.
    #[must_use]
    pub const fn hardening(&self) -> &HardeningConfig {
        &self.hardening
    }

    /// Capture the current immutable admission decision input.
    pub fn admission_snapshot(&self) -> Result<AdmissionSnapshotV1, PlatformError> {
        self.admission.snapshot(&self.data_dir)
    }

    /// Reserve conservative local bytes for one storage-growing operation.
    pub fn reserve_mutation(&self, bytes: u64) -> Result<AdmissionReservation, PlatformError> {
        self.admission.reserve(&self.data_dir, bytes)
    }

    /// Enter terminal draining mode and reject new storage-growing work.
    pub fn begin_draining(&self) {
        self.admission.begin_draining();
    }

    /// Filesystem block utilization containing the owned data directory, rounded down.
    pub fn filesystem_used_percent(&self) -> Result<u8, PlatformError> {
        let stat = rustix::fs::statvfs(self.data_dir.root()).map_err(|_| {
            PlatformError::new(
                open_compute_core::ErrorCode::DoStorageUnavailable,
                "Durable Object filesystem capacity is unavailable",
            )
        })?;
        if stat.f_blocks == 0 {
            return Err(PlatformError::new(
                open_compute_core::ErrorCode::DoStorageUnavailable,
                "Durable Object filesystem capacity is unavailable",
            ));
        }
        let used = stat.f_blocks.saturating_sub(stat.f_bfree);
        let percent = used.saturating_mul(100) / stat.f_blocks;
        u8::try_from(percent.min(100)).map_err(|_| {
            PlatformError::new(
                open_compute_core::ErrorCode::DoStorageUnavailable,
                "Durable Object filesystem capacity is unavailable",
            )
        })
    }
}

#[cfg(test)]
mod tests;
