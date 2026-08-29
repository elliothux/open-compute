//! Read-only validation of the current project-owned SQLite schema tuple.

use crate::scheduler::inspect_scheduler_schema_version;
use crate::{
    ControlDb, D1_DATABASE_SCHEMA_VERSION, D1Paths, DataDir, KV_SCHEMA_VERSION, KvPaths,
    current_scheduler_schema_version, migrations,
};
use open_compute_core::{AccountId, ErrorCode, PlatformError, ResourceId, ResourceState};
use rusqlite::{Connection, OpenFlags};
use serde::Serialize;
use std::path::Path;

/// Verified current schema identity and checked resource counts.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CurrentSchemaState {
    /// Current control schema version.
    pub control: u32,
    /// Current scheduler schema version.
    pub scheduler: u32,
    /// Current KV resource schema version.
    pub kv: u32,
    /// Current D1 resource schema version.
    pub d1: u32,
    /// Number of ready KV databases checked.
    pub kv_files: u32,
    /// Number of ready D1 databases checked.
    pub d1_files: u32,
}

/// Verify current schemas and resource files without applying or repairing any schema.
///
/// The caller owns the data-directory lock and supplies either its live control connection
/// or a WAL-aware read-only connection. Immutable SQLite reads would miss uncheckpointed state.
/// Creating resources and cancelled creates may not have a product catalog yet; deleting
/// resources may already have quarantined their files. Catalog identity is checked here, while
/// their product owner must reconcile physical state before serving traffic.
pub fn inspect_current_schema(
    data_dir: &DataDir,
    control_db: &ControlDb,
    busy_timeout_ms: u64,
) -> Result<CurrentSchemaState, PlatformError> {
    control_db.quick_check()?;
    let control = migrations::inspect_schema(control_db)?;
    let scheduler =
        inspect_scheduler_schema_version(&data_dir.scheduler_db_path(), busy_timeout_ms)?;
    if control != migrations::current_schema_version()
        || scheduler != current_scheduler_schema_version()
    {
        return Err(schema_invalid());
    }
    let resources = control_db.with_read(|connection| {
        let mut statement = connection
            .prepare(
                "SELECT r.kind, r.account_id, r.id, r.state, r.driver_schema_version,
                        COALESCE(k.storage_key, d.storage_key),
                        COALESCE(k.schema_version, d.schema_version)
                 FROM resources r
                 LEFT JOIN kv_namespaces k ON k.resource_id = r.id
                 LEFT JOIN d1_databases d ON d.resource_id = r.id
                 WHERE r.state != 'tombstoned' AND r.kind IN ('kv_namespace', 'd1_database')
                 ORDER BY r.kind, r.account_id, r.id",
            )
            .map_err(|_| schema_invalid())?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, Option<i64>>(6)?,
                ))
            })
            .map_err(|_| schema_invalid())?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|_| schema_invalid())
    })?;
    let mut state = CurrentSchemaState {
        control: u32::try_from(control).map_err(|_| schema_invalid())?,
        scheduler: u32::try_from(scheduler).map_err(|_| schema_invalid())?,
        kv: KV_SCHEMA_VERSION,
        d1: D1_DATABASE_SCHEMA_VERSION,
        kv_files: 0,
        d1_files: 0,
    };
    for (kind, account, resource, lifecycle, driver_version, storage_key, version) in resources {
        let account: AccountId = account.parse().map_err(|_| schema_invalid())?;
        let resource: ResourceId = resource.parse().map_err(|_| schema_invalid())?;
        let lifecycle: ResourceState = lifecycle.parse().map_err(|_| schema_invalid())?;
        let (product, expected_key, expected_version, count) = match kind.as_str() {
            "kv_namespace" => (
                "kv",
                KvPaths::storage_key(account, resource),
                state.kv,
                &mut state.kv_files,
            ),
            "d1_database" => (
                "d1",
                D1Paths::storage_key(account, resource),
                state.d1,
                &mut state.d1_files,
            ),
            _ => return Err(schema_invalid()),
        };
        if driver_version != i64::from(expected_version) {
            return Err(schema_invalid());
        }
        match (storage_key, version) {
            (None, None)
                if matches!(lifecycle, ResourceState::Creating | ResourceState::Deleting) =>
            {
                continue;
            }
            (Some(key), Some(version))
                if key == expected_key && version == i64::from(expected_version) => {}
            _ => return Err(schema_invalid()),
        }
        match lifecycle {
            ResourceState::Ready => {}
            ResourceState::Creating | ResourceState::Deleting => continue,
            ResourceState::Tombstoned => return Err(schema_invalid()),
        }
        let product_root = data_dir.root().join(product);
        let account_root = product_root.join(account.to_string());
        let resource_root = account_root.join(resource.to_string());
        for directory in [&product_root, &account_root, &resource_root] {
            crate::fs::validate_owned_dir(directory)?;
            crate::fs::validate_contained(data_dir.root(), directory)?;
        }
        sqlite_quick_check(&resource_root.join("data.sqlite"), busy_timeout_ms)?;
        *count = count.checked_add(1).ok_or_else(schema_invalid)?;
    }
    Ok(state)
}

fn sqlite_quick_check(path: &Path, busy_timeout_ms: u64) -> Result<(), PlatformError> {
    crate::fs::validate_owned_file(path, true)?;
    let open_path = crate::control_db::leaf_nofollow_path(path)?;
    let connection = Connection::open_with_flags(
        open_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .map_err(|_| schema_invalid())?;
    connection
        .busy_timeout(std::time::Duration::from_millis(busy_timeout_ms))
        .map_err(|_| schema_invalid())?;
    let value: String = connection
        .pragma_query_value(None, "quick_check", |row| row.get(0))
        .map_err(|_| schema_invalid())?;
    if value != "ok" {
        return Err(schema_invalid());
    }
    Ok(())
}

fn schema_invalid() -> PlatformError {
    PlatformError::new(
        ErrorCode::SchemaUnsupported,
        "persisted schema tuple does not match this implementation",
    )
}
