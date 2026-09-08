use super::*;

#[test]
fn migration_faults_checksum_future_and_restart() {
    let (_tmp, root) = unique_root();
    let config = storage_config(&root);
    for fault in [
        MigrationFault::BeforeExecution,
        MigrationFault::DuringDdl,
        MigrationFault::BeforeMigrationRow,
    ] {
        let (_t, r) = unique_root();
        let c = storage_config(&r);
        let err =
            PlatformStorage::bootstrap_with_fault(&c, &SystemClock, Some(fault)).expect_err("f");
        assert_eq!(err.code(), ErrorCode::MigrationFailed);
        assert_eq!(raw_user_version(&r.join("control.sqlite")), 0);
        PlatformStorage::bootstrap(&c, &SystemClock).expect("recover");
        assert_eq!(
            raw_user_version(&c.path.join("control.sqlite")),
            crate::migrations::current_schema_version()
        );
    }

    let err = PlatformStorage::bootstrap_with_fault(
        &config,
        &SystemClock,
        Some(MigrationFault::AfterCommit),
    )
    .expect_err("after commit reports failure");
    assert_eq!(err.code(), ErrorCode::MigrationFailed);
    assert_eq!(raw_user_version(&root.join("control.sqlite")), 1);
    PlatformStorage::bootstrap(&config, &SystemClock).expect("restart sees committed migration");

    let conn = Connection::open(root.join("control.sqlite")).unwrap();
    conn.execute(
        "UPDATE schema_migrations SET checksum_sha256 = ?1",
        [vec![0u8; 32]],
    )
    .unwrap();
    drop(conn);
    let checksum_err = PlatformStorage::bootstrap(&config, &SystemClock).expect_err("checksum");
    assert_eq!(checksum_err.code(), ErrorCode::MigrationFailed);
    let conn = Connection::open(root.join("control.sqlite")).unwrap();
    conn.pragma_update(None, "user_version", 99).unwrap();
    drop(conn);
    let err = PlatformStorage::bootstrap(&config, &SystemClock).expect_err("future");
    assert_eq!(err.code(), ErrorCode::SchemaTooNew);
}
