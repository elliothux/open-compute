use super::*;

#[test]
fn child_symlink_and_fifo_rejected() {
    let (_tmp, root) = unique_root();
    let config = storage_config(&root);
    let owned = DataDir::acquire(&config).expect("acquire");
    drop(owned);

    let outside = _tmp.path().join("outside");
    fs::write(&outside, b"x").unwrap();
    let keys = root.join("keys");
    fs::remove_dir_all(&keys).unwrap();
    std::os::unix::fs::symlink(&outside, &keys).unwrap();
    let err = DataDir::acquire(&config).expect_err("symlink child");
    assert_eq!(err.code(), ErrorCode::PathInvalid);
    fs::remove_file(&keys).unwrap();
    fs::create_dir(&keys).unwrap();
    fs::set_permissions(&keys, fs::Permissions::from_mode(0o700)).unwrap();

    let fifo = root.join("runtime").join("fifo");
    let status = Command::new("mkfifo").arg(&fifo).status().expect("mkfifo");
    assert!(status.success());
    let err = sfs::validate_contained(&root, &fifo).expect_err("fifo");
    assert_eq!(err.code(), ErrorCode::PathInvalid);
    let _ = fs::remove_file(&fifo);
}
