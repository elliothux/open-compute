use super::*;

#[test]
fn control_db_operations_fail_closed_when_foreign_keys_are_disabled() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("control.sqlite");
    let db = crate::ControlDb::open(&path, 100).unwrap();
    db.migrate(&SystemClock).unwrap();
    db.with_read(|conn| {
        conn.pragma_update(None, "foreign_keys", "OFF")
            .map_err(|_| open_compute_core::PlatformError::new(ErrorCode::Internal, "test"))?;
        Ok(())
    })
    .unwrap();

    assert_eq!(
        db.quick_check().unwrap_err().code(),
        ErrorCode::MigrationFailed
    );
    assert_eq!(
        db.user_version().unwrap_err().code(),
        ErrorCode::MigrationFailed
    );
    assert_eq!(
        db.with_read(|_| Ok(())).unwrap_err().code(),
        ErrorCode::MigrationFailed
    );
    assert_eq!(
        db.with_immediate(|_| Ok(())).unwrap_err().code(),
        ErrorCode::MigrationFailed
    );
    assert_eq!(
        db.with_exclusive(|_| Ok(())).unwrap_err().code(),
        ErrorCode::MigrationFailed
    );
    assert_eq!(
        db.table_exists("schema_migrations").unwrap_err().code(),
        ErrorCode::MigrationFailed
    );
    assert_eq!(
        db.migrate(&SystemClock).unwrap_err().code(),
        ErrorCode::MigrationFailed
    );
    assert_eq!(
        db.migrate_with_fault(&SystemClock, None)
            .unwrap_err()
            .code(),
        ErrorCode::MigrationFailed
    );
}
