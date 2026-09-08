use super::*;

#[test]
fn inspect_control_db_accepts_uri_special_path_chars() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("data?x#y%z");
    let config = storage_config(&root);
    drop(PlatformStorage::bootstrap(&config, &SystemClock).unwrap());
    let inspect = crate::inspect_data_root(&config).unwrap();
    assert!(inspect.lock_available);
    let (version, identity) =
        crate::inspect_control_db(&inspect.root.join("control.sqlite"), 5_000).unwrap();
    assert_eq!(version, crate::migrations::current_schema_version());
    assert!(!identity.master_key_id.is_empty());
}
