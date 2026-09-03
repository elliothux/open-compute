use super::*;
use crate::ResourceRecord;
use open_compute_core::{
    AccountId, BindingKind, D1Config, ResourceAvailability, ResourceId, ResourceState,
};
use std::sync::atomic::{AtomicU64, Ordering};

const QUOTA: u64 = 256 * 1024 * 1024;

fn limits() -> D1QueryLimits {
    D1QueryLimits::batch(&D1Config::default()).unwrap()
}

fn record(account: AccountId, resource: ResourceId) -> D1DatabaseRecord {
    D1DatabaseRecord {
        resource: ResourceRecord {
            id: resource,
            account_id: account,
            kind: BindingKind::D1Database,
            name: "transfer-source".to_owned(),
            state: ResourceState::Ready,
            availability: ResourceAvailability::Healthy,
            availability_code: None,
            spec_generation: 1,
            driver_schema_version: super::super::D1_DATABASE_SCHEMA_VERSION,
            created_at_ms: 10,
            updated_at_ms: 10,
            deleted_at_ms: None,
        },
        storage_key: "private".to_owned(),
        schema_version: super::super::D1_DATABASE_SCHEMA_VERSION,
        quota_bytes: QUOTA,
        last_opened_at_ms: None,
        last_quick_check_ms: None,
        last_backup_at_ms: None,
        restore_backup_id: None,
    }
}

#[test]
fn verified_sql_export_round_trips_through_fenced_import() {
    let temp = tempfile::tempdir().unwrap();
    let source_account = AccountId::generate();
    let source_resource = ResourceId::generate();
    let source = D1Engine::create(
        &temp.path().join("source.sqlite"),
        source_account,
        source_resource,
        10,
        QUOTA,
    )
    .unwrap();
    source
        .exec(
            "CREATE TABLE items(
               id INTEGER PRIMARY KEY,
               value TEXT,
               payload BLOB,
               derived TEXT GENERATED ALWAYS AS (value || '!') STORED
             );
             CREATE INDEX items_value ON items(value);
             INSERT INTO items(value, payload) VALUES ('it''s safe' || char(0) || 'tail', X'00ff');
             CREATE TABLE z_parent(id INTEGER PRIMARY KEY);
             CREATE TABLE a_child(parent_id INTEGER REFERENCES z_parent(id));
             INSERT INTO z_parent VALUES (7);
             INSERT INTO a_child VALUES (7)",
            limits(),
        )
        .unwrap();
    let snapshot = temp.path().join("snapshot.sqlite");
    source.online_backup(&snapshot).unwrap();
    let version = source.session_version().unwrap();
    let sql = D1Engine::export_sql(
        &snapshot,
        &record(source_account, source_resource),
        version,
        &D1ExportOptions::default(),
        super::super::D1_MAX_TRANSFER_SQL_BYTES,
    )
    .unwrap();
    let sql = String::from_utf8(sql).unwrap();
    assert!(!sql.contains("__open_compute_"));

    let schema_only = String::from_utf8(
        D1Engine::export_sql(
            &snapshot,
            &record(source_account, source_resource),
            version,
            &D1ExportOptions {
                no_schema: false,
                no_data: true,
                tables: ["items".to_owned()].into_iter().collect(),
            },
            super::super::D1_MAX_TRANSFER_SQL_BYTES,
        )
        .unwrap(),
    )
    .unwrap();
    assert!(schema_only.contains("CREATE TABLE items"));
    assert!(!schema_only.contains("INSERT INTO \"items\""));
    let data_only = String::from_utf8(
        D1Engine::export_sql(
            &snapshot,
            &record(source_account, source_resource),
            version,
            &D1ExportOptions {
                no_schema: true,
                no_data: false,
                tables: ["items".to_owned()].into_iter().collect(),
            },
            super::super::D1_MAX_TRANSFER_SQL_BYTES,
        )
        .unwrap(),
    )
    .unwrap();
    assert!(!data_only.contains("CREATE TABLE"));
    assert!(data_only.contains("INSERT INTO \"items\""));
    assert!(
        D1Engine::export_sql(
            &snapshot,
            &record(source_account, source_resource),
            version,
            &D1ExportOptions {
                no_schema: false,
                no_data: false,
                tables: ["missing".to_owned()].into_iter().collect(),
            },
            super::super::D1_MAX_TRANSFER_SQL_BYTES,
        )
        .is_err()
    );

    let destination = D1Engine::create(
        &temp.path().join("destination.sqlite"),
        AccountId::generate(),
        ResourceId::generate(),
        20,
        QUOTA,
    )
    .unwrap();
    let fenced_count = AtomicU64::new(0);
    let count = destination
        .import_sql(&sql, limits(), |result| {
            fenced_count.store(result.num_queries, Ordering::Release);
            Ok(())
        })
        .unwrap();
    assert_eq!(fenced_count.load(Ordering::Acquire), count.num_queries);
    assert!(count.num_queries >= 3);
    assert!(count.duration_ms >= 0.0);
    assert_eq!(count.rows_read, 0);
    assert!(count.rows_written >= 2);
    assert!(count.size_after > 0);
    assert_eq!(destination.session_version().unwrap(), 1);
    let connection = destination.open().unwrap();
    let row: (String, Vec<u8>, String) = connection
        .query_row("SELECT value, payload, derived FROM items", [], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })
        .unwrap();
    assert_eq!(
        row,
        (
            "it's safe\0tail".to_owned(),
            vec![0, 255],
            "it's safe\0tail!".to_owned()
        )
    );
    let child: i64 = connection
        .query_row("SELECT parent_id FROM a_child", [], |row| row.get(0))
        .unwrap();
    assert_eq!(child, 7);
}

#[test]
fn import_rolls_back_before_and_after_the_external_fence() {
    let temp = tempfile::tempdir().unwrap();
    let engine = D1Engine::create(
        &temp.path().join("database.sqlite"),
        AccountId::generate(),
        ResourceId::generate(),
        10,
        QUOTA,
    )
    .unwrap();
    let called = AtomicU64::new(0);
    assert!(
        engine
            .import_sql(
                "CREATE TABLE rolled_back(value TEXT); INSERT INTO missing VALUES (1)",
                limits(),
                |result| {
                    called.store(result.num_queries, Ordering::Release);
                    Ok(())
                },
            )
            .is_err()
    );
    assert_eq!(called.load(Ordering::Acquire), 0);
    assert_eq!(engine.session_version().unwrap(), 0);

    let rejected = PlatformError::new(ErrorCode::ResourceInvariantViolation, "fence rejected");
    assert_eq!(
        engine
            .import_sql("CREATE TABLE fenced(value TEXT)", limits(), |_| {
                Err(rejected.clone())
            })
            .unwrap_err()
            .code(),
        ErrorCode::ResourceInvariantViolation
    );
    assert_eq!(engine.session_version().unwrap(), 0);
    let connection = engine.open().unwrap();
    let tables: i64 = connection
        .query_row(
            "SELECT count(*) FROM sqlite_schema
             WHERE type = 'table' AND name IN ('rolled_back', 'fenced')",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(tables, 0);
}
