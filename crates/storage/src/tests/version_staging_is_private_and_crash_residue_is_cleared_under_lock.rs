use super::*;

#[test]
fn version_staging_is_private_and_crash_residue_is_cleared_under_lock() {
    let (_tmp, root) = unique_root();
    let config = storage_config(&root);
    let storage = PlatformStorage::bootstrap(&config, &SystemClock).expect("boot");
    let staging = storage.data_dir().version_staging_dir();
    assert_eq!(
        fs::metadata(&staging).unwrap().permissions().mode() & 0o777,
        0o700
    );
    drop(storage);

    let stale = staging.join("interrupted.upload");
    fs::write(&stale, b"partial tenant source").unwrap();
    fs::set_permissions(&stale, fs::Permissions::from_mode(0o600)).unwrap();
    let data_dir = DataDir::acquire(&config).expect("reacquire");
    assert!(!stale.exists());
    drop(data_dir);
}
