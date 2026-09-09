use super::*;

#[test]
fn control_db_read_write_helpers_and_failures_are_enforced() {
    let (_tmp, root) = unique_root();
    let storage = PlatformStorage::bootstrap(&storage_config(&root), &SystemClock).unwrap();
    let db = storage.db();
    assert!(db.table_exists("accounts").unwrap());
    assert!(!db.table_exists("not_a_table").unwrap());
    assert!(db.table_sql("accounts").unwrap().is_some());
    assert!(db.table_sql("not_a_table").unwrap().is_none());
    assert!(db.index_sql("not_an_index").unwrap().is_none());
    assert!(!db.dump_bytes().unwrap().is_empty());
    assert_eq!(
        db.pragma_display("user_version").unwrap(),
        crate::migrations::current_schema_version().to_string()
    );
    assert!(db.pragma_display("not_a_pragma").is_err());

    db.with_exclusive(|tx| {
        tx.execute(
            "INSERT INTO platform_meta (key, value, updated_at_ms) VALUES ('invalid_utf8', ?1, 1)",
            [vec![0xff]],
        )
        .map_err(|_| open_compute_core::PlatformError::new(ErrorCode::Internal, "insert"))?;
        Ok(())
    })
    .unwrap();
    assert_eq!(
        db.query_meta("invalid_utf8").unwrap_err().code(),
        ErrorCode::ConfigInvalid
    );
    assert!(db.query_meta("absent").unwrap().is_none());
    let expected = open_compute_core::PlatformError::new(ErrorCode::Internal, "callback");
    assert_eq!(
        db.with_immediate::<()>(|_| Err(expected.clone()))
            .unwrap_err()
            .code(),
        ErrorCode::Internal
    );

    let mut raw = Connection::open_in_memory().unwrap();
    raw.pragma_update(None, "foreign_keys", "OFF").unwrap();
    assert!(crate::control_db::verify_foreign_keys_on(&raw).is_err());
    raw.pragma_update(None, "foreign_keys", "ON").unwrap();
    crate::control_db::verify_foreign_keys_on(&raw).unwrap();
    let tx = raw.transaction().unwrap();
    crate::control_db::set_user_version(&tx, 7).unwrap();
    tx.commit().unwrap();
    assert_eq!(
        raw.pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
            .unwrap(),
        7
    );

    let db_path = root.join("control.sqlite");
    drop(storage);
    let readonly = crate::ControlDb::open_readonly(&db_path, 100).unwrap();
    assert_eq!(
        readonly.user_version().unwrap(),
        crate::migrations::current_schema_version()
    );
    readonly.quick_check().unwrap();
    assert!(crate::ControlDb::open_readonly(&root.join("missing.sqlite"), 100).is_err());
    assert!(crate::ControlDb::open(&root.join("missing/child.sqlite"), 100).is_err());
    let target = root.join("real.sqlite");
    fs::write(&target, b"").unwrap();
    let link = root.join("linked.sqlite");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    assert!(crate::ControlDb::open(&link, 100).is_err());
}
