//! Offline, forward-only P1 schema inspection and upgrade application.

use crate::{
    ControlDb, D1_DATABASE_SCHEMA_VERSION, DataDir, KV_SCHEMA_VERSION, SchedulerStore,
    current_scheduler_schema_version, inspect_control_db, inspect_scheduler_db, migrations,
};
use open_compute_core::clock::SystemClock;
use open_compute_core::{ErrorCode, PlatformError};
use rusqlite::{Connection, OpenFlags};
use serde::Serialize;
use std::path::Path;

/// Complete project-owned SQLite schema state for one stopped platform.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct OfflineSchemaState {
    /// Control schema version.
    pub control: u32,
    /// Scheduler schema version.
    pub scheduler: u32,
    /// Minimum catalogued KV schema, or current when no KV exists.
    pub kv_min: u32,
    /// Maximum catalogued KV schema, or current when no KV exists.
    pub kv_max: u32,
    /// Minimum catalogued D1 schema, or current when no D1 exists.
    pub d1_min: u32,
    /// Maximum catalogued D1 schema, or current when no D1 exists.
    pub d1_max: u32,
    /// Number of live KV databases checked.
    pub kv_files: u32,
    /// Number of live D1 databases checked.
    pub d1_files: u32,
}

/// Read and integrity-check the stopped platform without applying migrations.
pub fn inspect_offline_schema(
    data_dir: &DataDir,
    busy_timeout_ms: u64,
    now_ms: i64,
) -> Result<OfflineSchemaState, PlatformError> {
    let (control, _) = inspect_control_db(&data_dir.control_db_path(), busy_timeout_ms)?;
    let control_db =
        ControlDb::open_readonly_wal_aware(&data_dir.control_db_path(), busy_timeout_ms)?;
    inspect_schema_with_control(
        data_dir,
        &control_db,
        u32::try_from(control).map_err(|_| upgrade_invalid())?,
        busy_timeout_ms,
        now_ms,
    )
}

/// Inspect the currently owned platform using its live control connection.
///
/// This avoids an immutable read-only connection missing uncheckpointed WAL state during startup.
pub fn inspect_owned_schema(
    data_dir: &DataDir,
    control_db: &ControlDb,
    busy_timeout_ms: u64,
    now_ms: i64,
) -> Result<OfflineSchemaState, PlatformError> {
    control_db.quick_check()?;
    let control = migrations::inspect_schema(control_db)?;
    inspect_schema_with_control(
        data_dir,
        control_db,
        u32::try_from(control).map_err(|_| upgrade_invalid())?,
        busy_timeout_ms,
        now_ms,
    )
}

fn inspect_schema_with_control(
    data_dir: &DataDir,
    control_db: &ControlDb,
    control: u32,
    busy_timeout_ms: u64,
    now_ms: i64,
) -> Result<OfflineSchemaState, PlatformError> {
    let scheduler = inspect_scheduler_db(&data_dir.scheduler_db_path(), busy_timeout_ms, now_ms)?;
    let resources = control_db.with_read(|connection| {
        let mut statement = connection
            .prepare(
                "SELECT r.kind, r.account_id, r.id, COALESCE(k.storage_key, d.storage_key),
                        COALESCE(k.schema_version, d.schema_version)
                 FROM resources r
                 LEFT JOIN kv_namespaces k ON k.resource_id = r.id
                 LEFT JOIN d1_databases d ON d.resource_id = r.id
                 WHERE r.state != 'tombstoned' AND r.kind IN ('kv_namespace', 'd1_database')
                 ORDER BY r.kind, r.account_id, r.id",
            )
            .map_err(|_| upgrade_invalid())?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, i64>(4)?,
                ))
            })
            .map_err(|_| upgrade_invalid())?;
        let mut values = Vec::new();
        for row in rows {
            values.push(row.map_err(|_| upgrade_invalid())?);
        }
        Ok(values)
    })?;
    let mut kv_versions = Vec::new();
    let mut d1_versions = Vec::new();
    for (kind, account, resource, storage_key, version) in resources {
        if storage_key != format!("v1/{account}/{resource}/data.sqlite") {
            return Err(upgrade_invalid());
        }
        let product = match kind.as_str() {
            "kv_namespace" => "kv",
            "d1_database" => "d1",
            _ => return Err(upgrade_invalid()),
        };
        let path = data_dir
            .root()
            .join(product)
            .join(&account)
            .join(&resource)
            .join("data.sqlite");
        sqlite_quick_check(&path, busy_timeout_ms)?;
        let version = u32::try_from(version).map_err(|_| upgrade_invalid())?;
        if product == "kv" {
            kv_versions.push(version);
        } else {
            d1_versions.push(version);
        }
    }
    Ok(OfflineSchemaState {
        control,
        scheduler: u32::try_from(scheduler.schema_version).map_err(|_| upgrade_invalid())?,
        kv_min: kv_versions
            .iter()
            .copied()
            .min()
            .unwrap_or(KV_SCHEMA_VERSION),
        kv_max: kv_versions
            .iter()
            .copied()
            .max()
            .unwrap_or(KV_SCHEMA_VERSION),
        d1_min: d1_versions
            .iter()
            .copied()
            .min()
            .unwrap_or(D1_DATABASE_SCHEMA_VERSION),
        d1_max: d1_versions
            .iter()
            .copied()
            .max()
            .unwrap_or(D1_DATABASE_SCHEMA_VERSION),
        kv_files: u32::try_from(kv_versions.len()).map_err(|_| upgrade_invalid())?,
        d1_files: u32::try_from(d1_versions.len()).map_err(|_| upgrade_invalid())?,
    })
}

/// Apply all pending project-owned forward migrations and return the final checked state.
pub fn apply_offline_upgrade(
    data_dir: &DataDir,
    busy_timeout_ms: u64,
    now_ms: i64,
) -> Result<OfflineSchemaState, PlatformError> {
    let control = ControlDb::open(&data_dir.control_db_path(), busy_timeout_ms)?;
    migrations::apply(&control, &SystemClock)?;
    control.quick_check()?;
    drop(control);
    let scheduler = SchedulerStore::open(&data_dir.scheduler_db_path(), busy_timeout_ms, now_ms)?;
    scheduler.quick_check()?;
    drop(scheduler);
    let state = inspect_offline_schema(data_dir, busy_timeout_ms, now_ms)?;
    let target_control =
        u32::try_from(migrations::current_schema_version()).map_err(|_| upgrade_invalid())?;
    let target_scheduler =
        u32::try_from(current_scheduler_schema_version()).map_err(|_| upgrade_invalid())?;
    if state.control != target_control
        || state.scheduler != target_scheduler
        || state.kv_min != KV_SCHEMA_VERSION
        || state.kv_max != KV_SCHEMA_VERSION
        || state.d1_min != D1_DATABASE_SCHEMA_VERSION
        || state.d1_max != D1_DATABASE_SCHEMA_VERSION
    {
        return Err(upgrade_invalid());
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
    .map_err(|_| upgrade_invalid())?;
    connection
        .busy_timeout(std::time::Duration::from_millis(busy_timeout_ms))
        .map_err(|_| upgrade_invalid())?;
    let value: String = connection
        .pragma_query_value(None, "quick_check", |row| row.get(0))
        .map_err(|_| upgrade_invalid())?;
    if value != "ok" {
        return Err(upgrade_invalid());
    }
    Ok(())
}

fn upgrade_invalid() -> PlatformError {
    PlatformError::new(
        ErrorCode::UpgradeRequired,
        "offline schema tuple requires a supported forward upgrade",
    )
}
