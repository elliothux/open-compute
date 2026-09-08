use super::*;

#[test]
fn pragmas_schema_strict_and_partial_index() {
    let (_tmp, root) = unique_root();
    let config = storage_config(&root);
    let storage = PlatformStorage::bootstrap(&config, &SystemClock).expect("boot");
    assert_eq!(
        storage
            .db()
            .pragma_display("journal_mode")
            .unwrap()
            .to_lowercase(),
        "wal"
    );
    let sync = storage.db().pragma_display("synchronous").unwrap();
    assert!(sync == "2" || sync.eq_ignore_ascii_case("full"));
    assert_eq!(storage.db().pragma_display("foreign_keys").unwrap(), "1");
    assert_eq!(storage.db().pragma_display("trusted_schema").unwrap(), "0");
    for table in ["schema_migrations", "platform_meta", "accounts"] {
        let sql = storage.db().table_sql(table).unwrap().expect("sql");
        assert!(sql.to_ascii_uppercase().contains("STRICT"), "{sql}");
    }
    let idx = storage
        .db()
        .index_sql("accounts_live_name")
        .unwrap()
        .unwrap();
    assert!(idx.to_ascii_uppercase().contains("UNIQUE"));
    assert!(idx.contains("deleted_at_ms"));
}
