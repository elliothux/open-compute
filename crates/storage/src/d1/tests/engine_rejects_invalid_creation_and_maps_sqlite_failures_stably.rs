use super::*;

#[test]
fn engine_rejects_invalid_creation_and_maps_sqlite_failures_stably() {
    let temp = tempfile::tempdir().unwrap();
    assert_eq!(
        D1Engine::create(
            &temp.path().join("too-small.sqlite"),
            InstanceId::generate(),
            ResourceId::generate(),
            0,
            1024,
        )
        .unwrap_err()
        .code(),
        ErrorCode::D1IdentityMismatch
    );

    for (sqlite_code, expected) in [
        (rusqlite::ffi::SQLITE_CORRUPT, ErrorCode::D1DatabaseCorrupt),
        (rusqlite::ffi::SQLITE_NOTADB, ErrorCode::D1DatabaseCorrupt),
        (rusqlite::ffi::SQLITE_FULL, ErrorCode::D1DatabaseFull),
        (rusqlite::ffi::SQLITE_BUSY, ErrorCode::D1Overloaded),
        (rusqlite::ffi::SQLITE_LOCKED, ErrorCode::D1Overloaded),
        (
            rusqlite::ffi::SQLITE_CANTOPEN,
            ErrorCode::ResourceUnavailable,
        ),
    ] {
        let error = rusqlite::Error::SqliteFailure(rusqlite::ffi::Error::new(sqlite_code), None);
        assert_eq!(map_open_error(&error).code(), expected);
    }
    assert_eq!(
        map_internal_error(rusqlite::Error::InvalidQuery).code(),
        ErrorCode::D1DatabaseCorrupt
    );
}

#[test]
fn engine_rejects_old_account_metadata_without_rewriting_it() {
    let fixture = fixture();
    let connection = rusqlite::Connection::open(&fixture.engine.path).unwrap();
    connection
        .execute(
            "UPDATE __open_compute_meta SET key = 'account_id' WHERE key = 'instance_id'",
            [],
        )
        .unwrap();
    assert_eq!(
        fixture.engine.verify_identity().unwrap_err().code(),
        ErrorCode::D1IdentityMismatch
    );
    let legacy: Vec<u8> = connection
        .query_row(
            "SELECT value FROM __open_compute_meta WHERE key = 'account_id'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(legacy, fixture.account.to_string().as_bytes());
}
