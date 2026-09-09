use super::*;

#[test]
fn p0_2_migration_ddl_fault_rolls_back_to_schema_one() {
    let (_tmp, root) = unique_root();
    let config = storage_config(&root);
    let first = PlatformStorage::bootstrap_with_fault(
        &config,
        &SystemClock,
        Some(MigrationFault::AfterCommit),
    )
    .expect_err("migration one commits, then reports the injected fault");
    assert_eq!(first.code(), ErrorCode::MigrationFailed);
    assert_eq!(raw_user_version(&root.join("control.sqlite")), 1);

    let second = PlatformStorage::bootstrap_with_fault(
        &config,
        &SystemClock,
        Some(MigrationFault::DuringDdl),
    )
    .expect_err("migration two must roll back its entire trigger/table batch");
    assert_eq!(second.code(), ErrorCode::MigrationFailed);
    assert_eq!(raw_user_version(&root.join("control.sqlite")), 1);
    let conn = Connection::open(root.join("control.sqlite")).unwrap();
    let workers_exist: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='workers')",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(!workers_exist, "migration two DDL must be atomic");
    drop(conn);

    drop(PlatformStorage::bootstrap(&config, &SystemClock).unwrap());
    assert_eq!(
        raw_user_version(&root.join("control.sqlite")),
        crate::migrations::current_schema_version()
    );
}
