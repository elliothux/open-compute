//! Secure data-directory ownership, control database, identity, and secret crypto.

#![deny(missing_docs)]

pub mod bindings;
pub mod control_db;
pub mod crypto;
pub mod d1;
pub mod data_dir;
pub mod durable_objects;
pub mod fs;
pub mod identity;
pub mod inspect;
pub mod kv;
pub mod lock;
pub mod master_key;
pub mod migrations;
pub mod r2;
pub mod r2_staging;
pub mod resources;
pub mod scheduler;
pub mod workers;

pub use bindings::{
    AuthorizedBinding, BindingRepository, DeploymentBindingRecord, NewDeploymentBinding,
};
pub use control_db::ControlDb;
pub use crypto::{SecretCrypto, SecretEnvelope};
pub use d1::{
    D1_DATABASE_SCHEMA_VERSION, D1_MAX_BATCH_STATEMENTS, D1_MAX_BOUND_PARAMS, D1_MAX_COLUMNS,
    D1_MAX_EXEC_STATEMENTS, D1_MAX_SQL_BYTES, D1_MAX_VALUE_OR_ROW_BYTES, D1BackupRecord,
    D1BackupState, D1DatabaseRecord, D1DatabaseRepository, D1Engine, D1ExecResult, D1Meta,
    D1Migration, D1MigrationRecord, D1Paths, D1QueryLimits, D1Statement, D1StatementResult,
    D1Value,
};
pub use data_dir::{
    DURABLE_OBJECT_DATA_FORMAT_VERSION, DURABLE_OBJECT_UNIQUE_KEY, DataDir, expected_directories,
    future_resource_paths, inspect_durable_object_storage,
};
pub use durable_objects::{
    AuthorizedDurableObjectDelete, AuthorizedDurableObjectDispatch, DO_NAMESPACE_SCHEMA_VERSION,
    DurableObjectNamespaceRecord, DurableObjectRecord, DurableObjectRepository,
};
pub use fs::atomic_write;
pub use identity::{ARTIFACT_SCHEMA_VERSION, StableIdentity};
pub use inspect::{
    DataRootInspect, ResourceInspect, inspect_control_db, inspect_data_root, inspect_master_key,
    inspect_resources,
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
pub use r2::{R2_SCHEMA_VERSION, R2BucketRecord, R2BucketRepository};
pub use r2_staging::R2Staging;
pub use resources::{
    ReserveResourceCreate, ReserveResourceDelete, ResourceCreateReservation,
    ResourceDeleteReservation, ResourceRecord, ResourceReferrer, ResourceRepository,
};
pub use scheduler::{
    AlarmProjection, ClaimResult, ClaimedJob, SchedulerInspection, SchedulerStore,
    SchedulerSummary, inspect_scheduler_db,
};
pub use workers::{
    DeploymentRecord, DeploymentReferrer, DeploymentSnapshot, DeploymentState,
    IdempotencyReservation, LOADER_SCHEMA_VERSION, NewDeployment, RetentionCandidate, RouteKind,
    RouteRecord, RouteSnapshot, StoredDeploymentSecret, WorkerRecord, WorkerRepository,
};

use open_compute_core::PlatformError;
use open_compute_core::clock::Clock;
use open_compute_core::config::StorageConfig;

/// Fully bootstrapped P0.1 storage owner.
#[derive(Debug)]
pub struct PlatformStorage {
    data_dir: DataDir,
    db: ControlDb,
    crypto: SecretCrypto,
    identity: StableIdentity,
    free_space_hard_bytes: u64,
}

impl PlatformStorage {
    /// Acquire the data-dir lock, resolve the master key, then open/migrate the DB and identity.
    pub fn bootstrap(config: &StorageConfig, clock: &dyn Clock) -> Result<Self, PlatformError> {
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
        Ok(Self {
            data_dir,
            db,
            crypto,
            identity,
            free_space_hard_bytes: config.free_space_hard_bytes,
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
