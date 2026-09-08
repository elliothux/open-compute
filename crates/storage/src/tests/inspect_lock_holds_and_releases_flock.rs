use super::*;

#[test]
fn inspect_lock_holds_and_releases_flock() {
    let (_tmp, root) = unique_root();
    let config = storage_config(&root);
    drop(PlatformStorage::bootstrap(&config, &SystemClock).unwrap());
    let held = crate::InspectLock::try_acquire(&config.data_lock_path())
        .unwrap()
        .expect("available");
    assert!(!crate::DataDirLock::probe_available(&config.data_lock_path()).unwrap());
    drop(held);
    assert!(crate::DataDirLock::probe_available(&config.data_lock_path()).unwrap());
}
