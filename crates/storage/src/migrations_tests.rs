use super::*;
use open_compute_core::DeterministicClock;
use rusqlite::Connection;
use std::time::UNIX_EPOCH;

#[test]
fn fresh_control_uses_only_complete_refinery_history() {
    let directory = tempfile::tempdir().unwrap();
    let db = ControlDb::open(&directory.path().join("control.sqlite"), 100).unwrap();
    apply(&db, &DeterministicClock::new(UNIX_EPOCH)).unwrap();
    assert_eq!(inspect_schema(&db).unwrap(), current_schema_version());
    assert!(!db.table_exists("schema_migrations").unwrap());
    assert!(db.table_exists("refinery_schema_history").unwrap());
    assert_eq!(db.user_version().unwrap(), 0);
}

#[test]
fn exact_empty_refinery_history_recovers_the_first_migration_crash_boundary() {
    let directory = tempfile::tempdir().unwrap();
    let db = ControlDb::open(&directory.path().join("control.sqlite"), 100).unwrap();
    db.with_exclusive(|transaction| {
        transaction
            .execute_batch(
                "CREATE TABLE refinery_schema_history(
                   version int4 PRIMARY KEY,
                   name VARCHAR(255),
                   applied_on VARCHAR(255),
                   checksum VARCHAR(255)
                 );",
            )
            .map_err(|_| migration_failed())?;
        Ok(())
    })
    .unwrap();

    apply(&db, &DeterministicClock::new(UNIX_EPOCH)).unwrap();
    assert_eq!(inspect_schema(&db).unwrap(), current_schema_version());
}

#[test]
fn malformed_refinery_history_fails_closed() {
    let directory = tempfile::tempdir().unwrap();
    let db = ControlDb::open(&directory.path().join("control.sqlite"), 100).unwrap();
    apply(&db, &DeterministicClock::new(UNIX_EPOCH)).unwrap();
    db.with_exclusive(|transaction| {
        transaction
            .execute(
                "UPDATE refinery_schema_history SET checksum='not-a-checksum' WHERE version=1",
                [],
            )
            .map_err(|_| migration_failed())?;
        Ok(())
    })
    .unwrap();
    assert_eq!(
        inspect_schema(&db).unwrap_err().code(),
        ErrorCode::MigrationFailed
    );
}

#[test]
fn refinery_history_table_definition_is_part_of_the_schema_head() {
    let directory = tempfile::tempdir().unwrap();
    let db = ControlDb::open(&directory.path().join("control.sqlite"), 100).unwrap();
    apply(&db, &DeterministicClock::new(UNIX_EPOCH)).unwrap();
    db.with_exclusive(|transaction| {
        transaction
            .execute_batch(
                "ALTER TABLE refinery_schema_history RENAME TO old_refinery_schema_history;
                 CREATE TABLE refinery_schema_history(
                   version INTEGER PRIMARY KEY,
                   name TEXT,
                   applied_on TEXT,
                   checksum TEXT
                 );
                 INSERT INTO refinery_schema_history
                 SELECT * FROM old_refinery_schema_history;
                 DROP TABLE old_refinery_schema_history;",
            )
            .map_err(|_| migration_failed())?;
        Ok(())
    })
    .unwrap();
    assert_eq!(
        inspect_schema(&db).unwrap_err().code(),
        ErrorCode::MigrationFailed
    );
}

#[test]
fn refinery_history_cannot_mask_current_schema_drift() {
    let directory = tempfile::tempdir().unwrap();
    let db = ControlDb::open(&directory.path().join("control.sqlite"), 100).unwrap();
    apply(&db, &DeterministicClock::new(UNIX_EPOCH)).unwrap();
    db.with_exclusive(|transaction| {
        transaction
            .execute_batch("CREATE TABLE unexpected_platform_table(id INTEGER PRIMARY KEY) STRICT;")
            .map_err(|_| migration_failed())?;
        Ok(())
    })
    .unwrap();
    assert_eq!(
        inspect_schema(&db).unwrap_err().code(),
        ErrorCode::MigrationFailed
    );
}

#[test]
fn partial_refinery_head_drift_is_rejected_before_the_next_migration() {
    if current_schema_version() < 2 {
        return;
    }
    let directory = tempfile::tempdir().unwrap();
    let db = ControlDb::open(&directory.path().join("control.sqlite"), 100).unwrap();
    apply(&db, &DeterministicClock::new(UNIX_EPOCH)).unwrap();
    db.with_exclusive(|transaction| {
        transaction
            .execute_batch(
                "DELETE FROM refinery_schema_history WHERE version>=2;
                 ALTER TABLE worker_versions DROP COLUMN resource_limits_json;
                 CREATE TABLE unexpected_platform_table(id INTEGER PRIMARY KEY) STRICT;",
            )
            .map_err(|_| migration_failed())?;
        Ok(())
    })
    .unwrap();

    assert_eq!(
        apply(&db, &DeterministicClock::new(UNIX_EPOCH))
            .unwrap_err()
            .code(),
        ErrorCode::MigrationFailed
    );
    db.with_read(|connection| {
        let history: i64 = connection
            .query_row("SELECT COUNT(*) FROM refinery_schema_history", [], |row| {
                row.get(0)
            })
            .map_err(|_| migration_failed())?;
        let resource_limit_column: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('worker_versions')
                 WHERE name='resource_limits_json'",
                [],
                |row| row.get(0),
            )
            .map_err(|_| migration_failed())?;
        assert_eq!((history, resource_limit_column), (1, 0));
        Ok(())
    })
    .unwrap();
}

#[test]
fn committed_v1_schema_without_history_recovers_the_crash_between_transactions() {
    let directory = tempfile::tempdir().unwrap();
    let db = ControlDb::open(&directory.path().join("control.sqlite"), 100).unwrap();
    // Refinery commits each migration's SQL and its history row in separate transactions.
    // A process killed between them leaves the committed V1 schema without history and
    // without the legacy marker table; adoption must verify and install the baseline
    // instead of failing as an unverified legacy head.
    db.with_exclusive(|transaction| {
        transaction
            .execute_batch(include_str!("../refinery-migrations/control/V1__init.sql"))
            .map_err(|_| migration_failed())?;
        Ok(())
    })
    .unwrap();
    apply(&db, &DeterministicClock::new(UNIX_EPOCH)).unwrap();
    assert_eq!(inspect_schema(&db).unwrap(), current_schema_version());
    assert!(db.table_exists("refinery_schema_history").unwrap());
    assert!(!db.table_exists("schema_migrations").unwrap());
}

#[test]
fn local_extension_migration_rejects_old_service_descriptors_atomically() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("control.sqlite");
    let mut connection = Connection::open(&path).unwrap();
    schema_migrations::migrate_to_for_test(&mut connection, DatabaseKind::Control, 6);
    connection
        .pragma_update(None, "foreign_keys", "ON")
        .unwrap();
    connection
        .execute_batch(
            "INSERT INTO accounts(id, name, created_at_ms, deleted_at_ms)
             VALUES('00000000-0000-7000-8000-000000000001', 'account', 1, NULL);
             INSERT INTO workers(
               id, account_id, name, active_deployment_id, do_storage_id,
               route_generation, created_at_ms, updated_at_ms, deleted_at_ms, ownership
             ) VALUES(
               '00000000-0000-7000-8000-000000000002',
               '00000000-0000-7000-8000-000000000001', 'worker', NULL,
               '00000000-0000-7000-8000-000000000003', 0, 1, 1, NULL, 'tenant'
             );
             INSERT INTO worker_versions(
               id, worker_id, version_number, content_kind, state,
               artifact_sha256, artifact_size, artifact_schema_version, main_module,
               worker_code_sha256, loader_schema_version, compatibility_date,
               compatibility_flags_json, created_at_ms
             ) VALUES(
               '00000000-0000-7000-8000-000000000004',
               '00000000-0000-7000-8000-000000000002', 1, 'worker', 'staging',
               zeroblob(32), 1, 1, 'index.js', zeroblob(32), 1, '2026-09-08', X'5B5D', 1
             );
             INSERT INTO version_services(
               version_id, binding_name, target_worker_id, entrypoint,
               props_json, descriptor_sha256, created_at_ms
             ) VALUES(
               '00000000-0000-7000-8000-000000000004', 'SERVICE',
               '00000000-0000-7000-8000-000000000002', NULL, NULL, zeroblob(32), 1
             );",
        )
        .unwrap();
    drop(connection);

    let db = ControlDb::open(&path, 100).unwrap();
    assert_eq!(
        apply(&db, &DeterministicClock::new(UNIX_EPOCH))
            .unwrap_err()
            .code(),
        ErrorCode::MigrationFailed
    );
    db.with_read(|connection| {
        let state: (i64, i64, i64) = connection
            .query_row(
                "SELECT
                   (SELECT MAX(version) FROM refinery_schema_history),
                   (SELECT COUNT(*) FROM version_services),
                   (SELECT COUNT(*) FROM pragma_table_info('version_services')
                    WHERE name='target_kind')",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .map_err(|_| migration_failed())?;
        assert_eq!(state, (6, 1, 0));
        Ok(())
    })
    .unwrap();
}
