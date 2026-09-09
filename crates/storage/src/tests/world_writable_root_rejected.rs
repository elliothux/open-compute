use super::*;

#[test]
fn world_writable_root_rejected() {
    let (_tmp, root) = unique_root();
    fs::create_dir(&root).unwrap();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o777)).unwrap();
    let config = storage_config(&root);
    let err = DataDir::acquire(&config).expect_err("world");
    assert_eq!(err.code(), ErrorCode::PathInvalid);
    restore_writable(&root);
}
