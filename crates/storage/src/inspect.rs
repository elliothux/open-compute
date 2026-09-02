//! Read-only inspection of an existing data directory.

use crate::control_db::ControlDb;
use crate::fs;
use crate::identity::{self, StableIdentity};
use crate::lock::{DataDirLock, FilesystemDurability, InspectLock};
use crate::master_key::{self, MasterKey};
use crate::migrations;
use open_compute_core::SnapshotImmutableReferenceV1;
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

/// Fixed low-cardinality inventory used by platform metrics and diagnostics.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ControlInventory {
    /// Live accounts.
    pub accounts: u64,
    /// Live Workers.
    pub workers: u64,
    /// Non-tombstoned deployments.
    pub deployments: u64,
    /// Active routes.
    pub routes: u64,
    /// Non-tombstoned KV namespaces.
    pub kv_namespaces: u64,
    /// Non-tombstoned R2 buckets.
    pub r2_buckets: u64,
    /// Non-tombstoned D1 databases.
    pub d1_databases: u64,
    /// Non-tombstoned Durable Object namespaces.
    pub do_namespaces: u64,
    /// Non-tombstoned Vectorize indexes.
    pub vectorize_indexes: u64,
    /// Non-tombstoned AI Search namespaces.
    pub ai_search_namespaces: u64,
    /// Non-tombstoned AI Search instances.
    pub ai_search_instances: u64,
    /// Non-tombstoned Queues.
    pub queues: u64,
    /// Queues still creating their scheduler projection.
    pub queues_creating: u64,
    /// Queues converging deletion.
    pub queues_deleting: u64,
    /// Retired Queue tombstones.
    pub queues_tombstoned: u64,
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

/// Return only the aggregate count of bounded control audit events.
pub fn inspect_operator_event_count(
    path: &Path,
    busy_timeout_ms: u64,
) -> Result<u64, PlatformError> {
    let db = ControlDb::open_readonly(path, busy_timeout_ms)?;
    db.with_read(|connection| {
        let count: i64 = connection
            .query_row("SELECT COUNT(*) FROM control_audit_events", [], |row| {
                row.get(0)
            })
            .map_err(|_| inspect_error())?;
        u64::try_from(count).map_err(|_| inspect_error())
    })
}

/// Read aggregate live-object counts without returning tenant identifiers.
pub fn inspect_control_inventory(db: &ControlDb) -> Result<ControlInventory, PlatformError> {
    db.with_read(|connection| {
        Ok(ControlInventory {
            accounts: query_count(
                connection,
                "SELECT COUNT(*) FROM accounts WHERE deleted_at_ms IS NULL",
            )?,
            workers: query_count(
                connection,
                "SELECT COUNT(*) FROM workers WHERE deleted_at_ms IS NULL",
            )?,
            deployments: query_count(
                connection,
                "SELECT COUNT(*) FROM worker_deployments WHERE state != 'tombstoned'",
            )?,
            routes: query_count(
                connection,
                "SELECT COUNT(*) FROM worker_routes WHERE state = 'active'",
            )?,
            kv_namespaces: query_resource_count(connection, "kv_namespace")?,
            r2_buckets: query_resource_count(connection, "r2_bucket")?,
            d1_databases: query_resource_count(connection, "d1_database")?,
            do_namespaces: query_resource_count(connection, "do_namespace")?,
            vectorize_indexes: query_resource_count(connection, "vectorize_index")?,
            ai_search_namespaces: query_resource_count(connection, "ai_search_namespace")?,
            ai_search_instances: query_resource_count(connection, "ai_search_instance")?,
            queues: query_count(
                connection,
                "SELECT COUNT(*) FROM queues WHERE state != 'tombstoned'",
            )?,
            queues_creating: query_count(
                connection,
                "SELECT COUNT(*) FROM queues WHERE state = 'creating'",
            )?,
            queues_deleting: query_count(
                connection,
                "SELECT COUNT(*) FROM queues WHERE state = 'deleting'",
            )?,
            queues_tombstoned: query_count(
                connection,
                "SELECT COUNT(*) FROM queues WHERE state = 'tombstoned'",
            )?,
        })
    })
}

fn query_count(connection: &rusqlite::Connection, sql: &str) -> Result<u64, PlatformError> {
    let count: i64 = connection
        .query_row(sql, [], |row| row.get(0))
        .map_err(|_| inspect_error())?;
    u64::try_from(count).map_err(|_| inspect_error())
}

fn query_resource_count(
    connection: &rusqlite::Connection,
    kind: &str,
) -> Result<u64, PlatformError> {
    let count: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM resources WHERE kind = ?1 AND state != 'tombstoned'",
            [kind],
            |row| row.get(0),
        )
        .map_err(|_| inspect_error())?;
    u64::try_from(count).map_err(|_| inspect_error())
}

/// Enumerate immutable system objects referenced by live control authority.
pub fn inspect_snapshot_immutable_references(
    path: &Path,
    busy_timeout_ms: u64,
    system_prefix: &str,
) -> Result<Vec<SnapshotImmutableReferenceV1>, PlatformError> {
    if !system_prefix.ends_with('/') || system_prefix.contains("..") {
        return Err(inspect_error());
    }
    let db = ControlDb::open_readonly(path, busy_timeout_ms)?;
    db.with_read(|connection| {
        let mut references = Vec::new();
        let mut deployments = connection
            .prepare(
                "SELECT sha256, size FROM (
                   SELECT r.sha256 AS sha256, r.size AS size
                   FROM deployment_object_refs r
                   JOIN worker_deployments d ON d.id = r.deployment_id
                   WHERE d.state != 'tombstoned'
                   UNION
                   SELECT o.sha256 AS sha256, o.size AS size
                   FROM deployment_upload_objects o
                   JOIN deployment_uploads u ON u.id = o.session_id
                   WHERE u.status IN ('open', 'finalizing') AND o.verified = 1
                 ) ORDER BY sha256, size",
            )
            .map_err(|_| inspect_error())?;
        let rows = deployments
            .query_map([], |row| {
                Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, i64>(1)?))
            })
            .map_err(|_| inspect_error())?;
        for row in rows {
            let (digest, size) = row.map_err(|_| inspect_error())?;
            let sha256 = valid_digest(&digest)?;
            references.push(SnapshotImmutableReferenceV1 {
                role: "deployment_artifact".to_owned(),
                object_key: format!(
                    "{system_prefix}artifacts/v1/sha256/{}/{rest}",
                    &sha256[..2],
                    rest = &sha256[2..]
                ),
                sha256,
                size: u64::try_from(size).map_err(|_| inspect_error())?,
            });
        }
        for (table, role, prefix) in [
            ("kv_backups", "kv_backup", "backups/kv/"),
            ("d1_backups", "d1_backup", "backups/d1/"),
        ] {
            let sql = format!(
                "SELECT object_key, sha256, size_bytes FROM {table}
                 WHERE state = 'ready' ORDER BY object_key"
            );
            let mut statement = connection.prepare(&sql).map_err(|_| inspect_error())?;
            let rows = statement
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Vec<u8>>(1)?,
                        row.get::<_, i64>(2)?,
                    ))
                })
                .map_err(|_| inspect_error())?;
            for row in rows {
                let (object_key, digest, size) = row.map_err(|_| inspect_error())?;
                if !object_key.starts_with(&format!("{system_prefix}{prefix}"))
                    || object_key.contains("..")
                {
                    return Err(inspect_error());
                }
                references.push(SnapshotImmutableReferenceV1 {
                    role: role.to_owned(),
                    object_key,
                    sha256: valid_digest(&digest)?,
                    size: u64::try_from(size).map_err(|_| inspect_error())?,
                });
            }
        }
        references.sort_by(|left, right| {
            left.object_key
                .cmp(&right.object_key)
                .then(left.role.cmp(&right.role))
        });
        references.dedup();
        Ok(references)
    })
}

fn valid_digest(bytes: &[u8]) -> Result<String, PlatformError> {
    if bytes.len() != 32 {
        return Err(inspect_error());
    }
    Ok(hex::encode(bytes))
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
