use super::*;
use open_compute_core::ErrorCode;

#[test]
fn path_uri_foreign_key_and_transaction_failure_paths_are_stable() {
    let tmp = tempfile::tempdir().unwrap();
    assert_eq!(
        leaf_nofollow_path(Path::new("relative.sqlite"))
            .unwrap_err()
            .code(),
        ErrorCode::PathInvalid
    );
    assert_eq!(
        leaf_nofollow_path(&tmp.path().join("missing/control.sqlite"))
            .unwrap_err()
            .code(),
        ErrorCode::PathInvalid
    );
    let target = tmp.path().join("target.sqlite");
    std::fs::write(&target, b"").unwrap();
    let link = tmp.path().join("link.sqlite");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    assert_eq!(
        leaf_nofollow_path(&link).unwrap_err().code(),
        ErrorCode::PathInvalid
    );
    assert!(sqlite_readonly_uri(&tmp.path().join("a b?#.sqlite")).contains("%20"));

    let raw = Connection::open_in_memory().unwrap();
    raw.pragma_update(None, "foreign_keys", "OFF").unwrap();
    assert_eq!(
        verify_foreign_keys_on(&raw).unwrap_err().code(),
        ErrorCode::MigrationFailed
    );
    raw.pragma_update(None, "foreign_keys", "ON").unwrap();
    assert!(verify_foreign_keys_on(&raw).is_ok());

    let path = tmp.path().join("control.sqlite");
    let db = ControlDb::open(&path, 100).unwrap();
    let commit = db.with_exclusive(|tx| {
        tx.execute_batch("COMMIT").unwrap();
        Ok(())
    });
    assert_eq!(commit.unwrap_err().code(), ErrorCode::MigrationFailed);

    let path = tmp.path().join("immediate.sqlite");
    let db = ControlDb::open(&path, 100).unwrap();
    let commit = db.with_immediate(|tx| {
        tx.execute_batch("COMMIT").unwrap();
        Ok(())
    });
    assert_eq!(commit.unwrap_err().code(), ErrorCode::MigrationFailed);
}

#[test]
fn poisoned_connection_mutex_fails_closed() {
    let tmp = tempfile::tempdir().unwrap();
    let db = ControlDb::open(&tmp.path().join("control.sqlite"), 100).unwrap();
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = db.with_read::<()>(|_| panic!("poison control db for test"));
    }));
    assert_eq!(
        db.user_version().unwrap_err().code(),
        ErrorCode::MigrationFailed
    );
    assert_eq!(
        db.quick_check().unwrap_err().code(),
        ErrorCode::MigrationFailed
    );
}

#[test]
fn readonly_unmigrated_and_closed_transaction_failures_are_typed() {
    let temp = tempfile::tempdir().unwrap();
    assert_eq!(
        ControlDb::open_readonly(&temp.path().join("missing.sqlite"), 100)
            .unwrap_err()
            .code(),
        ErrorCode::MigrationFailed
    );
    assert_eq!(
        ControlDb::open(temp.path(), 100).unwrap_err().code(),
        ErrorCode::MigrationFailed
    );

    let path = temp.path().join("control.sqlite");
    let db = ControlDb::open(&path, 100).unwrap();
    assert_eq!(
        db.query_meta("missing").unwrap_err().code(),
        ErrorCode::MigrationFailed
    );
    assert_eq!(
        db.pragma_display("definitely_not_a_pragma")
            .unwrap_err()
            .code(),
        ErrorCode::MigrationFailed
    );
    db.with_exclusive(|tx| {
        tx.execute_batch("COMMIT").unwrap();
        Ok(())
    })
    .unwrap_err();
    drop(db);

    let readonly = ControlDb::open_readonly(&path, 100).unwrap();
    assert_eq!(
        readonly
            .with_immediate(|tx| set_user_version(tx, 1))
            .unwrap_err()
            .code(),
        ErrorCode::MigrationFailed
    );
    assert_eq!(
        readonly
            .with_exclusive(|tx| set_user_version(tx, 1))
            .unwrap_err()
            .code(),
        ErrorCode::MigrationFailed
    );
}
