use super::*;

#[test]
fn incomplete_identity_fails_without_mutation() {
    let (_tmp, root) = unique_root();
    let config = storage_config(&root);
    let first = PlatformStorage::bootstrap(&config, &SystemClock).unwrap();
    first.db().quick_check().unwrap();
    let last = first
        .db()
        .query_meta("last_started_version")
        .unwrap()
        .expect("last");
    let fp = first.identity().master_key_id.clone();
    drop(first);

    let conn = Connection::open(root.join("control.sqlite")).unwrap();
    conn.execute("DELETE FROM accounts", []).unwrap();
    drop(conn);
    let err = PlatformStorage::bootstrap(&config, &SystemClock).expect_err("missing account");
    assert_eq!(err.code(), ErrorCode::MigrationFailed);
    let conn = Connection::open(root.join("control.sqlite")).unwrap();
    let still: String = conn
        .query_row(
            "SELECT CAST(value AS TEXT) FROM platform_meta WHERE key = 'last_started_version'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(still, last);
    conn.execute(
        "INSERT INTO accounts (id, name, created_at_ms, deleted_at_ms) VALUES ('acct_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', 'default', 1, NULL)",
        [],
    )
    .ok();
    conn.execute("DELETE FROM platform_meta WHERE key = 'created_at_ms'", [])
        .unwrap();
    drop(conn);
    let err = PlatformStorage::bootstrap(&config, &SystemClock).expect_err("missing created");
    assert_eq!(err.code(), ErrorCode::MigrationFailed);

    let conn = Connection::open(root.join("control.sqlite")).unwrap();
    conn.execute(
        "INSERT INTO platform_meta (key, value, updated_at_ms) VALUES ('created_at_ms', CAST('1' AS BLOB), 1)",
        [],
    )
    .unwrap();
    conn.execute(
        "UPDATE platform_meta SET value = CAST('2' AS BLOB) WHERE key = 'artifact_schema_version'",
        [],
    )
    .unwrap();
    drop(conn);
    let err = PlatformStorage::bootstrap(&config, &SystemClock).expect_err("artifact");
    assert_eq!(err.code(), ErrorCode::MigrationFailed);
    let conn = Connection::open(root.join("control.sqlite")).unwrap();
    let stored_fp: Vec<u8> = conn
        .query_row(
            "SELECT value FROM platform_meta WHERE key = 'master_key_id'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(stored_fp, fp.as_bytes());
}
