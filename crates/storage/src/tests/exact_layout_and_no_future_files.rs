use super::*;

#[test]
fn exact_layout_and_no_future_files() {
    let (_tmp, root) = unique_root();
    let config = storage_config(&root);
    let storage = PlatformStorage::bootstrap(&config, &SystemClock).expect("boot");
    for dir in expected_directories(&root) {
        assert!(dir.is_dir(), "{}", dir.display());
        let mode = fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700, "{}", dir.display());
    }
    assert!(root.join("platform.lock").is_file());
    assert!(root.join("control.sqlite").is_file());
    for future in future_resource_paths(&root) {
        assert!(!future.exists(), "{}", future.display());
    }
    drop(storage);
}
