use super::*;

#[test]
fn sqlite_and_key_symlinks_are_rejected() {
    let (_tmp, root) = unique_root();
    let config = storage_config(&root);
    let _owned = DataDir::acquire(&config).unwrap();
    drop(_owned);
    let outside = _tmp.path().join("outside.sqlite");
    fs::write(&outside, b"x").unwrap();
    std::os::unix::fs::symlink(&outside, root.join("control.sqlite")).unwrap();
    let err = PlatformStorage::bootstrap(&config, &SystemClock).expect_err("db symlink");
    assert_eq!(err.code(), ErrorCode::PathInvalid);
    assert_eq!(fs::read(&outside).unwrap(), b"x");

    fs::remove_file(root.join("control.sqlite")).unwrap();
    let key_outside = _tmp.path().join("outside.key");
    fs::write(
        &key_outside,
        b"ocmk1:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
    )
    .unwrap();
    fs::create_dir_all(root.join("keys")).unwrap();
    fs::set_permissions(root.join("keys"), fs::Permissions::from_mode(0o700)).unwrap();
    std::os::unix::fs::symlink(&key_outside, &config.master_key_file).unwrap();
    let err = master_key::resolve(&config).expect_err("key symlink");
    assert_eq!(err.code(), ErrorCode::PathInvalid);
    assert_eq!(
        fs::read(&key_outside).unwrap(),
        b"ocmk1:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"
    );
}
