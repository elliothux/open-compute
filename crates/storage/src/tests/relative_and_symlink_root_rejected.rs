use super::*;

#[test]
fn relative_and_symlink_root_rejected() {
    let tmp = tempfile::tempdir().expect("tmp");
    let mut relative = storage_config(tmp.path());
    relative.path = PathBuf::from("relative-data");
    let err = DataDir::acquire(&relative).expect_err("relative");
    assert_eq!(err.code(), ErrorCode::PathInvalid);

    let real = tmp.path().join("real");
    fs::create_dir(&real).unwrap();
    fs::set_permissions(&real, fs::Permissions::from_mode(0o700)).unwrap();
    let link = tmp.path().join("link");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let mut cfg = storage_config(&link);
    cfg.master_key_file = link.join("keys/master.key");
    let err = DataDir::acquire(&cfg).expect_err("symlink root");
    assert_eq!(err.code(), ErrorCode::PathInvalid);
}
