use super::*;

#[test]
fn missing_master_key_env_fails_closed() {
    let (_tmp, root) = unique_root();
    let mut config = storage_config(&root);
    config.master_key_env = Some("PLATFORM_STORAGE_MISSING_KEY".to_string());
    master_key::clear_test_env();
    let _owned = DataDir::acquire(&config).unwrap();
    drop(_owned);
    let err = master_key::resolve(&config).expect_err("missing env");
    assert_eq!(err.code(), ErrorCode::MasterKeyMismatch);
    assert!(!root.join("control.sqlite").exists());
    assert!(!config.master_key_file.exists());
}
