use super::*;

#[test]
fn partially_created_key_is_rejected() {
    let (_tmp, root) = unique_root();
    let config = storage_config(&root);
    let _ = DataDir::acquire(&config).unwrap();
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&config.master_key_file)
        .unwrap();
    let err = master_key::resolve(&config).expect_err("empty key");
    assert_eq!(err.code(), ErrorCode::MasterKeyMismatch);
}
