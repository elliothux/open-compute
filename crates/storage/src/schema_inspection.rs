//! Validation of independently migrated project-owned SQLite databases.

use crate::scheduler::inspect_scheduler_schema_version;
use crate::schema_migrations::DatabaseKind;
use crate::{
    AI_SEARCH_SCHEMA_VERSION, AiSearchPaths, ControlDb, D1_DATABASE_SCHEMA_VERSION, D1Paths,
    DataDir, KV_SCHEMA_VERSION, KvPaths, VECTORIZE_SCHEMA_VERSION, VectorizePaths,
    current_scheduler_schema_version, migrations,
};
use open_compute_core::{ErrorCode, InstanceId, PlatformError, ResourceId, ResourceState};
use rusqlite::{Connection, OpenFlags};
use serde::Serialize;
use std::path::Path;

/// Counts of authoritative resource databases validated at their independent heads.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct SchemaInspection {
    /// Number of ready KV databases checked.
    pub kv_files: u32,
    /// Number of ready D1 databases checked.
    pub d1_files: u32,
    /// Number of ready Vectorize databases checked.
    pub vectorize_files: u32,
    /// Number of ready AI Search instance databases checked.
    pub ai_search_files: u32,
}

/// Migrate each cataloged resource file to its embedded head, then verify every schema.
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
) -> Result<SchemaInspection, PlatformError> {
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
                "SELECT r.kind, (SELECT instance_id FROM instance_identity), r.id, r.state, r.driver_schema_version,
                        COALESCE(k.storage_key, d.storage_key, v.storage_key, a.storage_key),
                        COALESCE(k.schema_version, d.schema_version, v.schema_version, a.schema_version)
                 FROM resources r
                 LEFT JOIN kv_namespaces k ON k.resource_id = r.id
                 LEFT JOIN d1_databases d ON d.resource_id = r.id
                 LEFT JOIN vectorize_indexes v ON v.resource_id = r.id
                 LEFT JOIN ai_search_instances a ON a.resource_id = r.id
                 WHERE r.state != 'tombstoned'
                   AND r.kind IN ('kv_namespace', 'd1_database', 'vectorize_index', 'ai_search_instance')
                 ORDER BY r.kind, (SELECT instance_id FROM instance_identity), r.id",
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
    let mut state = SchemaInspection {
        kv_files: 0,
        d1_files: 0,
        vectorize_files: 0,
        ai_search_files: 0,
    };
    for (kind, instance, resource, lifecycle, driver_version, storage_key, version) in resources {
        let instance: InstanceId = instance.parse().map_err(|_| schema_invalid())?;
        let resource: ResourceId = resource.parse().map_err(|_| schema_invalid())?;
        let lifecycle: ResourceState = lifecycle.parse().map_err(|_| schema_invalid())?;
        let (product, expected_key, expected_version, database_kind, count) = match kind.as_str() {
            "kv_namespace" => (
                "kv",
                KvPaths::storage_key(instance, resource),
                KV_SCHEMA_VERSION,
                DatabaseKind::Kv,
                &mut state.kv_files,
            ),
            "d1_database" => (
                "d1",
                D1Paths::storage_key(instance, resource),
                D1_DATABASE_SCHEMA_VERSION,
                DatabaseKind::D1,
                &mut state.d1_files,
            ),
            "vectorize_index" => (
                "vectorize",
                VectorizePaths::storage_key(instance, resource),
                VECTORIZE_SCHEMA_VERSION,
                DatabaseKind::Vectorize,
                &mut state.vectorize_files,
            ),
            "ai_search_instance" => (
                "ai-search",
                AiSearchPaths::storage_key(instance, resource),
                AI_SEARCH_SCHEMA_VERSION,
                DatabaseKind::AiSearch,
                &mut state.ai_search_files,
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
        let instance_root = product_root.join(instance.to_string());
        let resource_root = instance_root.join(resource.to_string());
        for directory in [&product_root, &instance_root, &resource_root] {
            crate::fs::validate_owned_dir(directory)?;
            crate::fs::validate_contained(data_dir.root(), directory)?;
        }
        sqlite_migrate_and_check(
            &resource_root.join("data.sqlite"),
            busy_timeout_ms,
            database_kind,
            instance,
            resource,
        )?;
        *count = count.checked_add(1).ok_or_else(schema_invalid)?;
    }
    Ok(state)
}

fn sqlite_migrate_and_check(
    path: &Path,
    busy_timeout_ms: u64,
    kind: DatabaseKind,
    instance: InstanceId,
    resource: ResourceId,
) -> Result<(), PlatformError> {
    crate::fs::validate_owned_file(path, true)?;
    let open_path = crate::control_db::leaf_nofollow_path(path)?;
    let mut connection = Connection::open_with_flags(
        open_path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .map_err(|_| schema_invalid())?;
    connection
        .busy_timeout(std::time::Duration::from_millis(busy_timeout_ms))
        .map_err(|_| schema_invalid())?;
    connection
        .pragma_update(None, "foreign_keys", "ON")
        .and_then(|()| connection.pragma_update(None, "trusted_schema", "OFF"))
        .map_err(|_| schema_invalid())?;
    verify_product_identity(&connection, kind, instance, resource)?;
    crate::schema_migrations::migrate(&mut connection, kind).map_err(|_| schema_invalid())?;
    let value: String = connection
        .pragma_query_value(None, "quick_check", |row| row.get(0))
        .map_err(|_| schema_invalid())?;
    if value != "ok" {
        return Err(schema_invalid());
    }
    Ok(())
}

fn verify_product_identity(
    connection: &Connection,
    kind: DatabaseKind,
    instance: InstanceId,
    resource: ResourceId,
) -> Result<(), PlatformError> {
    match kind {
        DatabaseKind::Kv | DatabaseKind::D1 => {
            let table = if kind == DatabaseKind::Kv {
                "kv_meta"
            } else {
                "__open_compute_meta"
            };
            for (key, expected) in [
                ("instance_id", instance.to_string()),
                ("resource_id", resource.to_string()),
            ] {
                let value: Vec<u8> = connection
                    .query_row(
                        &format!("SELECT value FROM {table} WHERE key=?1"),
                        [key],
                        |row| row.get(0),
                    )
                    .map_err(|_| schema_invalid())?;
                if value != expected.as_bytes() {
                    return Err(schema_invalid());
                }
            }
        }
        DatabaseKind::Vectorize | DatabaseKind::AiSearch => {
            let table = if kind == DatabaseKind::Vectorize {
                "index_meta"
            } else {
                "instance_meta"
            };
            let value: String = connection
                .query_row(
                    &format!("SELECT resource_id FROM {table} WHERE singleton=1"),
                    [],
                    |row| row.get(0),
                )
                .map_err(|_| schema_invalid())?;
            if value != resource.to_string() {
                return Err(schema_invalid());
            }
        }
        DatabaseKind::Control | DatabaseKind::Scheduler | DatabaseKind::Observability => {
            return Err(schema_invalid());
        }
    }
    Ok(())
}

fn schema_invalid() -> PlatformError {
    PlatformError::new(
        ErrorCode::SchemaUnsupported,
        "persisted SQLite schema does not match this implementation",
    )
}
