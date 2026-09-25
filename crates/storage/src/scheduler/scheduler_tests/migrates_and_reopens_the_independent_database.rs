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
    let store = SchedulerStore::open(
        &path,
        100,
        10,
        "019c0000000070008000000000000001".parse().unwrap(),
    )
    .unwrap();
    assert_eq!(store.summary(10).unwrap(), SchedulerSummary::default());
    drop(store);
    let reopened = SchedulerStore::open(
        &path,
        100,
        20,
        "019c0000000070008000000000000001".parse().unwrap(),
    )
    .unwrap();
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
        SchedulerStore::open(
            &path,
            100,
            30,
            "019c0000000070008000000000000001".parse().unwrap()
        )
        .unwrap_err()
        .code(),
        ErrorCode::SchedulerCorrupt
    );
}

#[test]
fn v1_schema_without_history_is_rejected_without_mutation() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("scheduler.sqlite");
    let connection = Connection::open(&path).unwrap();
    connection
        .execute_batch(include_str!(
            "../../../refinery-migrations/scheduler/V1__init.sql"
        ))
        .unwrap();
    drop(connection);
    assert_eq!(
        SchedulerStore::open(
            &path,
            100,
            10,
            "019c0000000070008000000000000001".parse().unwrap(),
        )
        .unwrap_err()
        .code(),
        ErrorCode::SchedulerCorrupt
    );
    let connection = Connection::open(&path).unwrap();
    let history: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE name='refinery_schema_history'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(history, 0);
}

#[test]
fn old_scheduler_history_is_rejected_without_rewriting_it() {
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

    assert_eq!(
        SchedulerStore::open(
            &path,
            100,
            10,
            "019c0000000070008000000000000001".parse().unwrap(),
        )
        .unwrap_err()
        .code(),
        ErrorCode::SchedulerCorrupt
    );
    let reopened = Connection::open(&path).unwrap();
    let marker: i64 = reopened
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='scheduler_migrations'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(marker, 1);
    let history: i64 = reopened
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE name='refinery_schema_history'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(history, 0);
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
        SchedulerStore::open(
            &path,
            100,
            10,
            "019c0000000070008000000000000001".parse().unwrap()
        )
        .unwrap_err()
        .code(),
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
        SchedulerStore::open(
            &drifted,
            100,
            10,
            "019c0000000070008000000000000001".parse().unwrap()
        )
        .unwrap_err()
        .code(),
        ErrorCode::SchedulerCorrupt
    );
}

#[test]
fn migrated_scheduler_owner_is_unique_and_mixed_projections_roll_back() {
    for mixed in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("scheduler.sqlite");
        let mut connection = Connection::open(&path).unwrap();
        crate::schema_migrations::migrate_to_for_test(
            &mut connection,
            crate::schema_migrations::DatabaseKind::Scheduler,
            1,
        );
        let owner = InstanceId::generate();
        for account_id in [owner, if mixed { InstanceId::generate() } else { owner }] {
            connection
                .execute(
                    "INSERT INTO cron_schedules
                     (activation_id,account_id,worker_id,version_id,execution_generation,
                      activation_generation,expression,expression_sha256,parser_version,
                      state,next_fire_at_ms,updated_at_ms)
                     VALUES(?1,?2,?3,?4,1,1,'* * * * *',?5,1,'accepting',100,1)",
                    params![
                        CronActivationId::generate().to_string(),
                        account_id.as_uuid().to_string(),
                        WorkerId::generate().to_string(),
                        VersionId::generate().to_string(),
                        [0u8; 32].as_slice(),
                    ],
                )
                .unwrap();
        }
        drop(connection);
        if mixed {
            assert_eq!(
                SchedulerStore::open(
                    &path,
                    100,
                    10,
                    "019c0000000070008000000000000001".parse().unwrap()
                )
                .unwrap_err()
                .code(),
                ErrorCode::SchedulerCorrupt
            );
            let connection = Connection::open(&path).unwrap();
            let identity_table: i64 = connection
                .query_row(
                    "SELECT count(*) FROM sqlite_master WHERE name='scheduler_identity'",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(identity_table, 0);
            continue;
        }
        drop(SchedulerStore::open(&path, 100, 10, owner).unwrap());
        let connection = Connection::open(&path).unwrap();
        let actual: String = connection
            .query_row(
                "SELECT instance_id FROM scheduler_identity WHERE singleton=1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(actual, owner.to_string());
        for table in ["queue_state", "cron_schedules", "workflow_instances"] {
            let columns = connection
                .prepare(&format!("PRAGMA table_info({table})"))
                .unwrap()
                .query_map([], |row| row.get::<_, String>(1))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap();
            assert!(!columns.iter().any(|column| column == "account_id"));
        }
    }
}

#[test]
fn scheduler_owner_is_bound_before_recovery_and_cannot_be_reassigned() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("scheduler.sqlite");
    let owner = InstanceId::generate();
    let other = InstanceId::generate();
    let store = SchedulerStore::open(&path, 100, 10, owner).unwrap();
    let namespace = ResourceId::generate();
    store
        .upsert_alarm(
            &projection(namespace, object(namespace, 7), "coverage-token-01", 20),
            10,
        )
        .unwrap();
    drop(store);
    assert_eq!(
        SchedulerStore::open(&path, 100, 20, other)
            .unwrap_err()
            .code(),
        ErrorCode::SchedulerCorrupt
    );
    drop(SchedulerStore::open(&path, 100, 20, owner).unwrap());
    let connection = Connection::open(&path).unwrap();
    connection
        .execute("DELETE FROM scheduler_identity", [])
        .unwrap();
    drop(connection);
    assert_eq!(
        SchedulerStore::open(&path, 100, 30, owner)
            .unwrap_err()
            .code(),
        ErrorCode::SchedulerCorrupt
    );
}
