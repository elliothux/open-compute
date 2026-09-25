use super::*;

#[tokio::test]
async fn uninstall_unregisters_owned_instance_and_preserves_data() {
    let temp = TempDir::new().unwrap();
    let bin_dir = temp.path().join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let binary_path = bin_dir.join("ocd");
    let binary = fake_binary("0.1.0");
    fs::write(&binary_path, &binary).unwrap();
    let receipt_path = temp.path().join("ocd/install-receipt.json");
    write_receipt(
        &receipt_path,
        &InstallReceipt {
            schema_version: RECEIPT_SCHEMA_VERSION,
            version: "0.1.0".to_owned(),
            sha256: hex::encode(Sha256::digest(&binary)),
            target: host_target().to_owned(),
            binary_path: binary_path.to_string_lossy().into_owned(),
            method: "install.sh".to_owned(),
            source: "test://current".to_owned(),
            installed_at_ms: 1,
        },
    )
    .unwrap();
    let registry = InstanceRegistry::with_roots(
        temp.path().join("registry/system"),
        temp.path().join("registry/user"),
    );
    let config_root = temp.path().join("instance");
    fs::create_dir_all(&config_root).unwrap();
    let config = write_loadable_config(&config_root);
    // Register via low-level write by using a canonical absolute path digest.
    let canonical = config.canonicalize().unwrap();
    registry
        .register(&canonical, ServiceScope::User, SystemTime::now())
        .unwrap();
    let manager = FakeServiceManager::default();
    manager
        .install(ServiceScope::User, None, &binary_path)
        .unwrap();
    let mut out = Vec::new();
    run_uninstall(
        &receipt_path,
        &binary_path,
        &registry,
        &manager,
        ServiceScope::User,
        UninstallOptions::default(),
        &mut out,
    )
    .unwrap();
    let output = String::from_utf8(out).unwrap();
    assert!(output.contains("UNINSTALL_RETAIN"));
    assert!(output.contains("INSTANCE_UNREGISTERED"));
    assert!(config.exists());
    assert!(!binary_path.exists());
    assert!(registry.list_scope(ServiceScope::User).unwrap().is_empty());
}

#[tokio::test]
async fn uninstall_unregisters_every_instance_in_the_daemon_scope() {
    let temp = TempDir::new().unwrap();
    let (binary_path, receipt_path, _) = write_upgradeable_pair(&temp, "0.1.0");
    let config_root = temp.path().join("instance");
    fs::create_dir_all(&config_root).unwrap();
    let config = write_loadable_config(&config_root);
    let registry = InstanceRegistry::with_roots(
        temp.path().join("registry/system"),
        temp.path().join("registry/user"),
    );
    registry
        .register(
            &config.canonicalize().unwrap(),
            ServiceScope::User,
            SystemTime::now(),
        )
        .unwrap();
    run_uninstall(
        &receipt_path,
        &binary_path,
        &registry,
        &FakeServiceManager::default(),
        ServiceScope::User,
        UninstallOptions::default(),
        &mut Vec::new(),
    )
    .unwrap();
    assert!(registry.list_scope(ServiceScope::User).unwrap().is_empty());
    assert!(config.exists());
}

#[tokio::test]
async fn uninstall_with_purge_deletes_owned_instance_state() {
    let temp = TempDir::new().unwrap();
    let (binary_path, receipt_path, _) = write_upgradeable_pair(&temp, "0.1.0");
    let config_root = temp.path().join("instance");
    fs::create_dir_all(&config_root).unwrap();
    let config = write_loadable_config(&config_root);
    let registry = InstanceRegistry::with_roots(
        temp.path().join("registry/system"),
        temp.path().join("registry/user"),
    );
    registry
        .register(
            &config.canonicalize().unwrap(),
            ServiceScope::User,
            SystemTime::now(),
        )
        .unwrap();
    let manager = FakeServiceManager::default();
    manager
        .install(ServiceScope::User, None, &binary_path)
        .unwrap();
    run_uninstall(
        &receipt_path,
        &binary_path,
        &registry,
        &manager,
        ServiceScope::User,
        UninstallOptions {
            purge: true,
            yes: true,
            dry_run: false,
        },
        &mut Vec::new(),
    )
    .unwrap();
    assert!(!config.exists());
    assert!(!config_root.join("data").exists());
    assert!(!binary_path.exists());
    assert!(!receipt_path.exists());
}

#[tokio::test]
async fn uninstall_preserves_and_reports_state_when_stopped_config_is_missing() {
    let temp = TempDir::new().unwrap();
    let (binary_path, receipt_path, _) = write_upgradeable_pair(&temp, "0.1.0");
    let config_root = temp.path().join("instance");
    fs::create_dir_all(&config_root).unwrap();
    let config = write_loadable_config(&config_root);
    let registry = InstanceRegistry::with_roots(
        temp.path().join("registry/system"),
        temp.path().join("registry/user"),
    );
    registry
        .register(
            &config.canonicalize().unwrap(),
            ServiceScope::User,
            SystemTime::now(),
        )
        .unwrap();
    fs::remove_file(&config).unwrap();
    let mut out = Vec::new();
    let error = run_uninstall(
        &receipt_path,
        &binary_path,
        &registry,
        &FakeServiceManager::default(),
        ServiceScope::User,
        UninstallOptions::default(),
        &mut out,
    )
    .unwrap_err();
    assert_eq!(error.code(), ErrorCode::InstanceRegistryInvalid);
    let data = config.parent().unwrap().join("data");
    assert!(data.exists());
    assert!(binary_path.exists() && receipt_path.exists());
    assert!(out.is_empty());
}
