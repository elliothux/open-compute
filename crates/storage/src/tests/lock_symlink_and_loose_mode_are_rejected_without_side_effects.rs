use super::*;

#[test]
fn lock_symlink_and_loose_mode_are_rejected_without_side_effects() {
    let tmp = tempfile::tempdir().expect("tmp");
    let root = tmp.path().join("data");
    fs::create_dir(&root).unwrap();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    let outside = tmp.path().join("outside.lock");
    fs::write(&outside, b"outside-target").unwrap();
    fs::set_permissions(&outside, fs::Permissions::from_mode(0o644)).unwrap();
    std::os::unix::fs::symlink(&outside, root.join("platform.lock")).unwrap();
    let config = storage_config(&root);
    let err = DataDir::acquire(&config).expect_err("symlink lock");
    assert_eq!(err.code(), ErrorCode::PathInvalid);
    assert_eq!(fs::read(&outside).unwrap(), b"outside-target");
    assert_eq!(
        fs::metadata(&outside).unwrap().permissions().mode() & 0o777,
        0o644
    );

    fs::remove_file(root.join("platform.lock")).unwrap();
    let lock_path = root.join("platform.lock");
    let mut loose = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o622)
        .open(&lock_path)
        .unwrap();
    loose.write_all(b"loose").unwrap();
    drop(loose);
    fs::set_permissions(&lock_path, fs::Permissions::from_mode(0o622)).unwrap();
    let err = DataDir::acquire(&config).expect_err("loose lock");
    assert_eq!(err.code(), ErrorCode::PathInvalid);
    assert_eq!(fs::read(&lock_path).unwrap(), b"loose");
    assert_eq!(
        fs::metadata(&lock_path).unwrap().permissions().mode() & 0o777,
        0o622
    );
}
