use super::*;

#[test]
fn committed_refinery_head_is_not_replayed_after_a_reported_failure() {
    let (_tmp, root) = unique_root();
    let config = storage_config(&root);
    let first = PlatformStorage::bootstrap_with_fault(
        &config,
        &SystemClock,
        Some(MigrationFault::AfterCommit),
    )
    .expect_err("migration one commits, then reports the injected fault");
    assert_eq!(first.code(), ErrorCode::MigrationFailed);
    assert_eq!(raw_user_version(&root.join("control.sqlite")), 0);

    let conn = Connection::open(root.join("control.sqlite")).unwrap();
    let workers_exist: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='workers')",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(
        workers_exist,
        "the complete Refinery head was already committed"
    );
    drop(conn);

    drop(PlatformStorage::bootstrap(&config, &SystemClock).unwrap());
    assert_eq!(raw_user_version(&root.join("control.sqlite")), 0);
}
