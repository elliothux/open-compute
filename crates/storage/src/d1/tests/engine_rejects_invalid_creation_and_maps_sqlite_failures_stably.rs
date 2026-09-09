use super::*;

#[test]
fn engine_rejects_invalid_creation_and_maps_sqlite_failures_stably() {
    let temp = tempfile::tempdir().unwrap();
    assert_eq!(
        D1Engine::create(
            &temp.path().join("too-small.sqlite"),
            AccountId::generate(),
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
