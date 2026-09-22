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

#[test]
fn extension_provider_directories_are_stable_and_enumerable() {
    let (_tmp, root) = unique_root();
    let data_dir = DataDir::acquire(&storage_config(&root)).unwrap();
    let beta = data_dir.prepare_extension_provider_dir("beta").unwrap();
    let alpha = data_dir.prepare_extension_provider_dir("alpha").unwrap();
    assert_eq!(
        data_dir.existing_extension_provider_dirs().unwrap(),
        vec![("alpha".into(), alpha), ("beta".into(), beta)]
    );
}

#[test]
fn gateway_directories_are_private_and_reject_symlinks() {
    let (_tmp, root) = unique_root();
    let config = storage_config(&root);
    let data_dir = DataDir::acquire(&config).unwrap();
    assert!(!root.join("gateway").exists());
    let gateway = data_dir.prepare_gateway_dir().unwrap();
    assert_eq!(gateway, root.join("gateway"));
    for path in [
        gateway.clone(),
        gateway.join("run"),
        gateway.join("storage"),
        gateway.join("config-state"),
    ] {
        assert_eq!(
            fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }
    let run = gateway.join("run");
    fs::remove_dir(&run).unwrap();
    std::os::unix::fs::symlink(root.join("runtime"), &run).unwrap();
    assert!(data_dir.prepare_gateway_dir().is_err());
    drop(data_dir);
    assert!(DataDir::acquire(&config).is_err());
}
