//! Read-only inspection of an existing data directory.

use crate::control_db::ControlDb;
use crate::fs;
use crate::identity::{self, StableIdentity};
use crate::lock::{DataDirLock, FilesystemDurability, InspectLock};
use crate::master_key::{self, MasterKey};
use crate::migrations;
use open_compute_core::config::StorageConfig;
use open_compute_core::{BindingKind, ErrorCode, PlatformError, ResourceAvailability, ResourceId};
use std::path::{Path, PathBuf};
use std::str::FromStr;

/// Snapshot of an existing data root. Never creates files or takes ownership.
#[derive(Debug)]
pub struct DataRootInspect {
    /// Absolute root.
    pub root: PathBuf,
    /// Filesystem durability classification.
    pub durability: FilesystemDurability,
    /// Available bytes from `statvfs`, when known.
    pub free_bytes: Option<u64>,
    /// Whether this inspect session holds the exclusive data lock.
    pub lock_available: bool,
    /// Exclusive inspect flock; dropped with this snapshot.
    inspect_lock: Option<InspectLock>,
}

/// Secret-free resource health row for operator inspection.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResourceInspect {
    /// Logical resource identity.
    pub id: ResourceId,
    /// Static product kind.
    pub kind: BindingKind,
    /// Persisted probe-derived availability.
    pub availability: ResourceAvailability,
    /// Stable resource-local health code.
    pub availability_code: Option<String>,
}

impl DataRootInspect {
    /// True when this snapshot still holds the inspect flock.
    #[must_use]
    pub fn holds_inspect_lock(&self) -> bool {
        self.inspect_lock.is_some()
    }
}

/// Inspect an existing data directory without creating layout, keys, or locks.
pub fn inspect_data_root(config: &StorageConfig) -> Result<DataRootInspect, PlatformError> {
    let root = &config.data_dir;
    fs::require_absolute(root)?;
    fs::validate_root(root)?;
    let lock_path = config.data_lock_path();
    fs::validate_contained(root, &lock_path)?;
    if !lock_path.exists() {
        return Err(PlatformError::new(
            ErrorCode::PathInvalid,
            "data directory lock file is missing",
        ));
    }
    let inspect_lock = InspectLock::try_acquire(&lock_path)?;
    let lock_available = inspect_lock.is_some();
    Ok(DataRootInspect {
        root: root.clone(),
        durability: DataDirLock::classify_path(root),
        free_bytes: free_bytes(root),
        lock_available,
        inspect_lock,
    })
}

/// Resolve an existing master key and return its fingerprint. Never generates a key.
pub fn inspect_master_key(config: &StorageConfig) -> Result<MasterKey, PlatformError> {
    master_key::inspect_existing(config)
}

/// Open `control.sqlite` read-only, run `quick_check`, and inspect schema/identity.
pub fn inspect_control_db(
    path: &Path,
    busy_timeout_ms: u64,
) -> Result<(i64, StableIdentity), PlatformError> {
    if !path.is_absolute() {
        return Err(PlatformError::new(
            ErrorCode::PathInvalid,
            "storage path must be an absolute path",
        ));
    }
    if !path.exists() {
        return Err(PlatformError::new(
            ErrorCode::PathInvalid,
            "control database is missing",
        ));
    }
    let db = ControlDb::open_readonly(path, busy_timeout_ms)?;
    db.quick_check()?;
    let version = migrations::inspect_schema(&db)?;
    let identity = identity::inspect_stored(&db)?;
    Ok((version, identity))
}

/// Read the bounded, secret-free resource health catalog from an existing database.
pub fn inspect_resources(
    path: &Path,
    busy_timeout_ms: u64,
    limit: u32,
) -> Result<Vec<ResourceInspect>, PlatformError> {
    if limit == 0 || limit > 10_000 {
        return Err(PlatformError::new(
            ErrorCode::LimitInvalid,
            "resource inspection limit is invalid",
        ));
    }
    let db = ControlDb::open_readonly(path, busy_timeout_ms)?;
    db.with_read(|connection| {
        let mut statement = connection
            .prepare(
                "SELECT id, kind, availability, availability_code
                 FROM resources WHERE state != 'tombstoned' ORDER BY kind, id LIMIT ?1",
            )
            .map_err(|_| inspect_error())?;
        let rows = statement
            .query_map([i64::from(limit)], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                ))
            })
            .map_err(|_| inspect_error())?;
        let mut resources = Vec::new();
        for row in rows {
            let (id, kind, availability, availability_code) = row.map_err(|_| inspect_error())?;
            resources.push(ResourceInspect {
                id: ResourceId::from_str(&id).map_err(|_| inspect_error())?,
                kind: BindingKind::from_str(&kind).map_err(|_| inspect_error())?,
                availability: ResourceAvailability::from_str(&availability)
                    .map_err(|_| inspect_error())?,
                availability_code,
            });
        }
        Ok(resources)
    })
}

fn inspect_error() -> PlatformError {
    PlatformError::new(
        ErrorCode::ResourceInvariantViolation,
        "resource health catalog is invalid",
    )
}

fn free_bytes(path: &Path) -> Option<u64> {
    let stat = rustix::fs::statvfs(path).ok()?;
    Some(stat.f_bavail.saturating_mul(stat.f_frsize))
}
