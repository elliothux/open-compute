//! P1 host-disk admission and process-local reservation ownership.

use crate::DataDir;
use open_compute_core::{
    AdmissionReservation, AdmissionReservations, AdmissionSnapshotV1, ErrorCode, HardeningConfig,
    PlatformError, PlatformMode, StorageConfig,
};
use std::path::Path;
use std::sync::atomic::{AtomicU8, Ordering};

const MODE_SERVING: u8 = 0;
const MODE_DRAINING: u8 = 1;
const MODE_OFFLINE: u8 = 2;
const MAX_STAGING_ENTRIES: u64 = 1_000_000;

/// Platform-wide admission authority for operations that may grow local state.
#[derive(Debug)]
pub struct DiskAdmission {
    soft_reserve_bytes: u64,
    hard_reserve_bytes: u64,
    emergency_reserve_bytes: u64,
    reservations: AdmissionReservations,
    mode: AtomicU8,
}

impl DiskAdmission {
    /// Construct the serving-mode authority from validated configuration.
    #[must_use]
    pub fn new(storage: &StorageConfig, hardening: &HardeningConfig) -> Self {
        Self {
            soft_reserve_bytes: storage.free_space_soft_bytes,
            hard_reserve_bytes: storage.free_space_hard_bytes,
            emergency_reserve_bytes: hardening.emergency_reserve_bytes,
            reservations: AdmissionReservations::default(),
            mode: AtomicU8::new(MODE_SERVING),
        }
    }

    /// Capture one immutable decision input from current host and staging state.
    pub fn snapshot(&self, data_dir: &DataDir) -> Result<AdmissionSnapshotV1, PlatformError> {
        let stat = rustix::fs::statvfs(data_dir.root()).map_err(|_| {
            PlatformError::new(
                ErrorCode::StoragePressure,
                "data directory free space could not be measured",
            )
        })?;
        let owned_staging_bytes = [
            data_dir.deployment_staging_dir(),
            data_dir.backup_staging_dir(),
            data_dir.root().join("r2-staging"),
        ]
        .iter()
        .try_fold(0_u64, |total, path| {
            Ok::<_, PlatformError>(total.saturating_add(owned_tree_bytes(path)?))
        })?;
        Ok(AdmissionSnapshotV1 {
            schema_version: 1,
            filesystem_free_bytes: stat.f_bavail.saturating_mul(stat.f_frsize),
            soft_reserve_bytes: self.soft_reserve_bytes,
            hard_reserve_bytes: self.hard_reserve_bytes,
            emergency_reserve_bytes: self.emergency_reserve_bytes,
            reserved_bytes: self.reservations.bytes(),
            owned_staging_bytes,
            mode: self.mode(),
        })
    }

    /// Atomically reserve bytes, then validate one post-reservation immutable snapshot.
    pub fn reserve(
        &self,
        data_dir: &DataDir,
        bytes: u64,
    ) -> Result<AdmissionReservation, PlatformError> {
        if bytes == 0 {
            return Err(PlatformError::new(
                ErrorCode::LimitInvalid,
                "admission reservation must be greater than zero",
            ));
        }
        let reservation = self.reservations.reserve(bytes)?;
        self.snapshot(data_dir)?.admit(0)?;
        Ok(reservation)
    }

    /// Enter the terminal draining mode. It cannot transition back to serving.
    pub fn begin_draining(&self) {
        let _ = self.mode.compare_exchange(
            MODE_SERVING,
            MODE_DRAINING,
            Ordering::AcqRel,
            Ordering::Acquire,
        );
    }

    /// Construct an offline-mode authority for an exclusive command owner.
    #[must_use]
    pub fn offline(storage: &StorageConfig, hardening: &HardeningConfig) -> Self {
        let value = Self::new(storage, hardening);
        value.mode.store(MODE_OFFLINE, Ordering::Release);
        value
    }

    fn mode(&self) -> PlatformMode {
        match self.mode.load(Ordering::Acquire) {
            MODE_SERVING => PlatformMode::Serving,
            MODE_DRAINING => PlatformMode::Draining,
            _ => PlatformMode::Offline,
        }
    }
}

fn owned_tree_bytes(root: &Path) -> Result<u64, PlatformError> {
    let metadata = match std::fs::symlink_metadata(root) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(_) => return Err(staging_invalid()),
    };
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(staging_invalid());
    }
    let mut pending = vec![root.to_path_buf()];
    let mut entries = 0_u64;
    let mut bytes = 0_u64;
    while let Some(directory) = pending.pop() {
        let read = std::fs::read_dir(&directory).map_err(|_| staging_invalid())?;
        for entry in read {
            let entry = entry.map_err(|_| staging_invalid())?;
            entries = entries.saturating_add(1);
            if entries > MAX_STAGING_ENTRIES {
                return Err(staging_invalid());
            }
            let metadata =
                std::fs::symlink_metadata(entry.path()).map_err(|_| staging_invalid())?;
            let kind = metadata.file_type();
            if kind.is_symlink() {
                return Err(staging_invalid());
            }
            if kind.is_dir() {
                pending.push(entry.path());
            } else if kind.is_file() {
                bytes = bytes
                    .checked_add(metadata.len())
                    .ok_or_else(staging_invalid)?;
            } else {
                return Err(staging_invalid());
            }
        }
    }
    Ok(bytes)
}

fn staging_invalid() -> PlatformError {
    PlatformError::new(
        ErrorCode::StoragePressure,
        "owned staging directory failed validation",
    )
}
