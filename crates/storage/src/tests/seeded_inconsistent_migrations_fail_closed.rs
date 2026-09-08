use super::*;

#[test]
fn seeded_inconsistent_migrations_fail_closed() {
    let (_tmp, root) = unique_root();
    let config = storage_config(&root);
    PlatformStorage::bootstrap(&config, &SystemClock).unwrap();

    let conn = Connection::open(root.join("control.sqlite")).unwrap();
    conn.execute("DELETE FROM schema_migrations", []).unwrap();
    drop(conn);
    let err = PlatformStorage::bootstrap(&config, &SystemClock).expect_err("missing rows");
    assert_eq!(err.code(), ErrorCode::MigrationFailed);

    let (_t2, root2) = unique_root();
    let c2 = storage_config(&root2);
    PlatformStorage::bootstrap(&c2, &SystemClock).unwrap();
    let conn = Connection::open(root2.join("control.sqlite")).unwrap();
    conn.pragma_update(None, "user_version", 0).unwrap();
    drop(conn);
    let err = PlatformStorage::bootstrap(&c2, &SystemClock).expect_err("uv0 with rows");
    assert_eq!(err.code(), ErrorCode::MigrationFailed);

    let (_t3, root3) = unique_root();
    let c3 = storage_config(&root3);
    PlatformStorage::bootstrap(&c3, &SystemClock).unwrap();
    let conn = Connection::open(root3.join("control.sqlite")).unwrap();
    conn.execute(
        "INSERT INTO schema_migrations (version, name, checksum_sha256, applied_at_ms, app_version)
         VALUES (99, 'future', ?1, 1, 'x')",
        [vec![0u8; 32]],
    )
    .unwrap();
    drop(conn);
    let err = PlatformStorage::bootstrap(&c3, &SystemClock).expect_err("row too new");
    assert_eq!(err.code(), ErrorCode::SchemaTooNew);
}
