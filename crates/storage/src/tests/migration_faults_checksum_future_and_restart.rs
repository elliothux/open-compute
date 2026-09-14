use super::*;

fn history_count(path: &Path) -> i64 {
    Connection::open(path)
        .unwrap()
        .query_row("SELECT COUNT(*) FROM refinery_schema_history", [], |row| {
            row.get(0)
        })
        .unwrap()
}

#[test]
fn migration_faults_checksum_future_and_restart() {
    {
        let (_temp, root) = unique_root();
        let config = storage_config(&root);
        let error = PlatformStorage::bootstrap_with_fault(
            &config,
            &SystemClock,
            Some(MigrationFault::BeforeExecution),
        )
        .expect_err("fault must precede migration commit");
        assert_eq!(error.code(), ErrorCode::MigrationFailed);
        assert_eq!(raw_user_version(&root.join("control.sqlite")), 0);
        PlatformStorage::bootstrap(&config, &SystemClock).unwrap();
        assert_eq!(
            history_count(&root.join("control.sqlite")),
            crate::migrations::current_schema_version()
        );
    }

    let (_temp, root) = unique_root();
    let config = storage_config(&root);
    let error = PlatformStorage::bootstrap_with_fault(
        &config,
        &SystemClock,
        Some(MigrationFault::AfterCommit),
    )
    .expect_err("post-commit fault must preserve the committed head");
    assert_eq!(error.code(), ErrorCode::MigrationFailed);
    assert_eq!(
        history_count(&root.join("control.sqlite")),
        crate::migrations::current_schema_version()
    );
    PlatformStorage::bootstrap(&config, &SystemClock).unwrap();

    let connection = Connection::open(root.join("control.sqlite")).unwrap();
    connection
        .execute(
            "UPDATE refinery_schema_history SET checksum='0' WHERE version=1",
            [],
        )
        .unwrap();
    drop(connection);
    assert_eq!(
        PlatformStorage::bootstrap(&config, &SystemClock)
            .unwrap_err()
            .code(),
        ErrorCode::MigrationFailed
    );

    let (_future_temp, future_root) = unique_root();
    let future_config = storage_config(&future_root);
    drop(PlatformStorage::bootstrap(&future_config, &SystemClock).unwrap());
    let connection = Connection::open(future_root.join("control.sqlite")).unwrap();
    connection
        .execute(
            "INSERT INTO refinery_schema_history(version,name,applied_on,checksum)
             VALUES(?1,'future','1970-01-01T00:00:00Z','0')",
            [crate::migrations::current_schema_version() + 1],
        )
        .unwrap();
    drop(connection);
    assert_eq!(
        PlatformStorage::bootstrap(&future_config, &SystemClock)
            .unwrap_err()
            .code(),
        ErrorCode::SchemaTooNew
    );
}
