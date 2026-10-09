//! Secure data-directory ownership, control database, identity, and secret crypto.

#![deny(missing_docs)]

pub mod ai_search;
pub mod assets;
pub mod bindings;
pub mod browser;
pub mod cache;
pub mod catalog_page;
pub mod cloudflare_artifacts;
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
pub mod observability;
#[cfg(test)]
#[path = "observability_tests.rs"]
mod observability_tests;
pub mod platform_restore;
pub mod platform_snapshot;
pub mod public_gateway;
pub mod queue_consumers;
pub mod queues;
pub mod r2;
pub mod r2_multipart;
pub mod r2_objects;
pub mod r2_staging;
pub mod resources;
pub mod restore_cleanup;
pub mod runtime_features;
pub mod scheduler;
pub mod schema_inspection;
mod schema_migrations;
pub mod services;
pub mod snapshot_staging;
pub mod vectorize;
pub mod worker_repository;
pub mod workflows;
use crate::control_db::ControlDb;
use crate::crypto::SecretCrypto;
use crate::data_dir::DataDir;
use crate::disk_admission::DiskAdmission;
use crate::identity::StableIdentity;
#[cfg(any(test, feature = "test-support"))]
use crate::migrations::MigrationFault;
use open_compute_core::clock::Clock;
use open_compute_core::config::DataConfig;
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
    sqlite_busy_timeout_ms: u64,
}

impl PlatformStorage {
    /// Verify an existing data layout and run pending control migrations on an in-memory snapshot.
    pub fn preflight_upgrade(config: &DataConfig, clock: &dyn Clock) -> Result<(), PlatformError> {
        fs::validate_root(&config.path)?;
        ControlDb::preflight_migrations(
            &config.path.join("control.sqlite"),
            config.sqlite_busy_timeout_ms,
            clock,
        )
    }

    /// Acquire the data-dir lock, resolve the master key, then open/migrate the DB and identity.
    pub fn bootstrap(config: &DataConfig, clock: &dyn Clock) -> Result<Self, PlatformError> {
        let mut hardening = HardeningConfig::default();
        hardening.emergency_reserve_bytes = hardening
            .emergency_reserve_bytes
            .min(config.free_space_hard_bytes.saturating_sub(1));
        Self::bootstrap_with_hardening(config, &hardening, clock)
    }

    /// Bootstrap with the P1 platform-wide hardening policy.
    pub fn bootstrap_with_hardening(
        config: &DataConfig,
        hardening: &HardeningConfig,
        clock: &dyn Clock,
    ) -> Result<Self, PlatformError> {
        let data_dir = DataDir::acquire(config)?;
        let key = master_key::resolve(config)?;
        let db_path = data_dir.ensure_control_db()?;
        let db = ControlDb::open(&db_path, config.sqlite_busy_timeout_ms)?;
        db.migrate(clock)?;
        let identity = identity::bootstrap(&db, clock, key.fingerprint())?;
        data_dir.record_instance_id(&identity.instance_id.to_string())?;
        let crypto = SecretCrypto::new(key.bytes(), key.fingerprint())?;
        Ok(Self {
            data_dir,
            db,
            crypto,
            identity,
            free_space_hard_bytes: config.free_space_hard_bytes,
            hardening: hardening.clone(),
            admission: DiskAdmission::new(config, hardening),
            sqlite_busy_timeout_ms: config.sqlite_busy_timeout_ms,
        })
    }

    /// Bootstrap with optional test-only migration fault injection.
    #[cfg(any(test, feature = "test-support"))]
    pub fn bootstrap_with_fault(
        config: &DataConfig,
        clock: &dyn Clock,
        fault: Option<MigrationFault>,
    ) -> Result<Self, PlatformError> {
        let data_dir = DataDir::acquire(config)?;
        let key = master_key::resolve(config)?;
        let db_path = data_dir.ensure_control_db()?;
        let db = ControlDb::open(&db_path, config.sqlite_busy_timeout_ms)?;
        db.migrate_with_fault(clock, fault)?;
        let identity = identity::bootstrap(&db, clock, key.fingerprint())?;
        data_dir.record_instance_id(&identity.instance_id.to_string())?;
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
            sqlite_busy_timeout_ms: config.sqlite_busy_timeout_ms,
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

    /// Establish or validate the immutable object-authority binding in SQLite.
    pub fn bind_object_authority(
        &self,
        kind: open_compute_core::ObjectStorageKind,
        authority_sha256: &[u8; 32],
    ) -> Result<(), PlatformError> {
        let now_ms = open_compute_core::wall_time_ms();
        identity::bind_object_authority(&self.db, kind, authority_sha256, now_ms)
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

    /// SQLite busy timeout shared by product databases.
    #[must_use]
    pub const fn sqlite_busy_timeout_ms(&self) -> u64 {
        self.sqlite_busy_timeout_ms
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
mod catalog_page_tests;
#[cfg(test)]
mod tests;
