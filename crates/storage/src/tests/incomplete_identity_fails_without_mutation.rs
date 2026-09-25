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
    let id = first.identity().instance_id.to_string();
    let created = first.identity().created_at_ms;
    drop(first);

    let conn = Connection::open(root.join("control.sqlite")).unwrap();
    conn.execute("DELETE FROM instance_identity", []).unwrap();
    drop(conn);
    let err = PlatformStorage::bootstrap(&config, &SystemClock).expect_err("missing instance");
    assert_eq!(err.code(), ErrorCode::ConfigInvalid);
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
        "INSERT INTO instance_identity (instance_id, created_at_ms) VALUES (?1, ?2)",
        rusqlite::params![id, created],
    )
    .unwrap();
    conn.execute("DELETE FROM platform_meta WHERE key = 'master_key_id'", [])
        .unwrap();
    drop(conn);
    let err = PlatformStorage::bootstrap(&config, &SystemClock).expect_err("missing master key id");
    assert_eq!(err.code(), ErrorCode::MigrationFailed);

    let conn = Connection::open(root.join("control.sqlite")).unwrap();
    conn.execute(
        "INSERT INTO platform_meta (key, value, updated_at_ms) VALUES ('master_key_id', ?1, 1)",
        [fp.as_bytes()],
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
