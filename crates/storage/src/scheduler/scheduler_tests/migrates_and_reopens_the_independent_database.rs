use super::*;

#[test]
fn migrates_and_reopens_the_independent_database() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("scheduler.sqlite");
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .unwrap();
    let store = SchedulerStore::open(&path, 100, 10).unwrap();
    assert_eq!(store.summary(10).unwrap(), SchedulerSummary::default());
    drop(store);
    let reopened = SchedulerStore::open(&path, 100, 20).unwrap();
    reopened.quick_check().unwrap();
    drop(reopened);
    let connection = Connection::open(&path).unwrap();
    connection
        .execute(
            "UPDATE refinery_schema_history SET checksum = '0' WHERE version = 1",
            [],
        )
        .unwrap();
    drop(connection);
    assert_eq!(
        SchedulerStore::open(&path, 100, 30).unwrap_err().code(),
        ErrorCode::SchedulerCorrupt
    );
}

#[test]
fn committed_v1_schema_without_history_recovers_the_crash_between_transactions() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("scheduler.sqlite");
    // Refinery commits the migration SQL and the history row in separate transactions, so a
    // killed first start can leave the committed V1 schema without history and without the
    // legacy marker table; reopening must adopt the verified baseline instead of reporting
    // corruption.
    let connection = Connection::open(&path).unwrap();
    connection
        .execute_batch(include_str!(
            "../../../refinery-migrations/scheduler/V1__init.sql"
        ))
        .unwrap();
    drop(connection);
    let store = SchedulerStore::open(&path, 100, 10).unwrap();
    store.quick_check().unwrap();
    assert_eq!(store.summary(10).unwrap(), SchedulerSummary::default());
}

#[test]
fn legacy_head_is_adopted_and_reshaped_on_reopen() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("scheduler.sqlite");
    // Rebuild exactly the pre-Refinery head: the five published legacy migrations with
    // their frozen identities, applied in order to a fresh database.
    let connection = Connection::open(&path).unwrap();
    for sql in [
        include_str!("../../../scheduler-migrations/001_scheduler.sql"),
        include_str!("../../../scheduler-migrations/002_queue_producer.sql"),
        include_str!("../../../scheduler-migrations/003_queue_consumer.sql"),
        include_str!("../../../scheduler-migrations/004_cron.sql"),
        include_str!("../../../scheduler-migrations/005_workflow.sql"),
    ] {
        connection.execute_batch(sql).unwrap();
    }
    connection
        .execute(
            "INSERT INTO scheduler_meta(singleton, schema_version, data_format, created_at_ms,
             updated_at_ms) VALUES(1, 0, ?1, 0, 0)",
            params![DATA_FORMAT],
        )
        .unwrap();
    let registry = scheduler_migration_registry();
    for (index, (version, name, checksum)) in registry.iter().enumerate() {
        connection
            .execute(
                "INSERT INTO scheduler_migrations(version, name, checksum_sha256, applied_at_ms,
                 app_version) VALUES(?1, ?2, ?3, ?4, ?5)",
                params![version, name, checksum.as_slice(), index as i64, "test"],
            )
            .unwrap();
    }
    connection
        .execute(
            "UPDATE scheduler_meta SET schema_version = ?1, data_format = ?2 WHERE singleton = 1",
            params![registry.len() as i64, DATA_FORMAT],
        )
        .unwrap();
    drop(connection);

    let store = SchedulerStore::open(&path, 100, 10).unwrap();
    store.quick_check().unwrap();
    assert_eq!(store.summary(10).unwrap(), SchedulerSummary::default());
    // The legacy marker table is reshaped away by adoption.
    let reopened = Connection::open(&path).unwrap();
    let marker: i64 = reopened
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='scheduler_migrations'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(marker, 0);
    let history: i64 = reopened
        .query_row("SELECT COUNT(*) FROM refinery_schema_history", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(history, 1);
}

#[test]
fn legacy_head_with_drifted_identity_fails_closed_on_reopen() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("scheduler.sqlite");
    let connection = Connection::open(&path).unwrap();
    for sql in [
        include_str!("../../../scheduler-migrations/001_scheduler.sql"),
        include_str!("../../../scheduler-migrations/002_queue_producer.sql"),
        include_str!("../../../scheduler-migrations/003_queue_consumer.sql"),
        include_str!("../../../scheduler-migrations/004_cron.sql"),
        include_str!("../../../scheduler-migrations/005_workflow.sql"),
    ] {
        connection.execute_batch(sql).unwrap();
    }
    connection
        .execute(
            "INSERT INTO scheduler_meta(singleton, schema_version, data_format, created_at_ms,
             updated_at_ms) VALUES(1, 0, ?1, 0, 0)",
            params![DATA_FORMAT],
        )
        .unwrap();
    let registry = scheduler_migration_registry();
    for (index, (version, name, checksum)) in registry.iter().enumerate() {
        connection
            .execute(
                "INSERT INTO scheduler_migrations(version, name, checksum_sha256, applied_at_ms,
                 app_version) VALUES(?1, ?2, ?3, ?4, ?5)",
                params![version, name, checksum.as_slice(), index as i64, "test"],
            )
            .unwrap();
    }
    // The recorded schema version trails the published legacy head.
    connection
        .execute(
            "UPDATE scheduler_meta SET schema_version = ?1 WHERE singleton = 1",
            params![(registry.len() - 1) as i64],
        )
        .unwrap();
    drop(connection);
    assert_eq!(
        SchedulerStore::open(&path, 100, 10).unwrap_err().code(),
        ErrorCode::SchedulerCorrupt
    );

    // A checksum drift in the applied history also fails closed.
    let drifted = temp.path().join("drifted.sqlite");
    std::fs::copy(&path, &drifted).unwrap();
    let connection = Connection::open(&drifted).unwrap();
    connection
        .execute(
            "UPDATE scheduler_meta SET schema_version = ?1 WHERE singleton = 1",
            params![registry.len() as i64],
        )
        .unwrap();
    connection
        .execute(
            "UPDATE scheduler_migrations SET checksum_sha256 = ?1 WHERE version = 1",
            params![[0u8; 32].as_slice()],
        )
        .unwrap();
    drop(connection);
    assert_eq!(
        SchedulerStore::open(&drifted, 100, 10).unwrap_err().code(),
        ErrorCode::SchedulerCorrupt
    );
}
