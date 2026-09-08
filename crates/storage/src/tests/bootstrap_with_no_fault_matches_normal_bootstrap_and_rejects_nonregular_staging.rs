use super::*;

#[test]
fn bootstrap_with_no_fault_matches_normal_bootstrap_and_rejects_nonregular_staging() {
    let (_tmp, root) = unique_root();
    let config = storage_config(&root);
    let storage = PlatformStorage::bootstrap_with_fault(&config, &SystemClock, None).unwrap();
    let staging = storage.data_dir().version_staging_dir();
    drop(storage);

    fs::create_dir(staging.join("nested")).unwrap();
    assert_eq!(
        DataDir::acquire(&config).unwrap_err().code(),
        ErrorCode::PathInvalid
    );
    fs::remove_dir(staging.join("nested")).unwrap();

    let target = _tmp.path().join("outside");
    fs::write(&target, b"outside").unwrap();
    std::os::unix::fs::symlink(&target, staging.join("link")).unwrap();
    assert_eq!(
        DataDir::acquire(&config).unwrap_err().code(),
        ErrorCode::PathInvalid
    );
}
