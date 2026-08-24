//! Secure data-directory ownership, control database, identity, and secret crypto.

#![deny(missing_docs)]

pub mod control_db;
pub mod crypto;
pub mod data_dir;
pub mod fs;
pub mod identity;
pub mod inspect;
pub mod lock;
pub mod master_key;
pub mod migrations;
pub mod workers;

pub use control_db::ControlDb;
pub use crypto::{SecretCrypto, SecretEnvelope};
pub use data_dir::{DataDir, expected_directories, future_resource_paths};
pub use fs::atomic_write;
pub use identity::{ARTIFACT_SCHEMA_VERSION, StableIdentity};
pub use inspect::{DataRootInspect, inspect_control_db, inspect_data_root, inspect_master_key};
pub use lock::{DataDirLock, FilesystemDurability, InspectLock};
pub use master_key::MasterKey;
#[cfg(any(test, feature = "test-support"))]
pub use master_key::{clear_test_env, set_test_env};
#[cfg(any(test, feature = "test-support"))]
pub use migrations::MigrationFault;
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
}

#[cfg(test)]
mod tests;
