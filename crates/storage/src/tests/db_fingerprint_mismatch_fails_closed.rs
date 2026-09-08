use super::*;

#[test]
fn db_fingerprint_mismatch_fails_closed() {
    let (_tmp, root) = unique_root();
    let config = storage_config(&root);
    let first = PlatformStorage::bootstrap(&config, &SystemClock).expect("first");
    drop(first);
    fs::remove_file(&config.master_key_file).unwrap();
    let err = PlatformStorage::bootstrap(&config, &SystemClock).expect_err("new key vs db");
    assert_eq!(err.code(), ErrorCode::MasterKeyMismatch);
    assert!(config.master_key_file.exists());
}
