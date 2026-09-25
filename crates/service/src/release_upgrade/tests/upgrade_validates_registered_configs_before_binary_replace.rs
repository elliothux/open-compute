use super::*;

#[tokio::test]
async fn upgrade_rejects_unloadable_registered_configuration() {
    let temp = TempDir::new().unwrap();
    let (binary_path, receipt_path, current) = write_upgradeable_pair(&temp, "0.1.0");
    let download_base = "https://fixture.test/download".to_owned();
    let api_base = "https://fixture.test/api".to_owned();
    let http = FixtureReleaseHttp::default();
    let target_version = env!("CARGO_PKG_VERSION");
    fixture_release(
        &http,
        &download_base,
        &api_base,
        target_version,
        host_target(),
        &fake_binary(target_version),
    );
    let registry = InstanceRegistry::with_roots(
        temp.path().join("registry/system"),
        temp.path().join("registry/user"),
    );
    let missing_root = temp.path().join("missing");
    fs::create_dir(&missing_root).unwrap();
    let config = write_loadable_config(&missing_root);
    registry
        .register(
            &config.canonicalize().unwrap(),
            ServiceScope::User,
            SystemTime::now(),
        )
        .unwrap();
    let changed_root = temp.path().join("changed");
    fs::create_dir(&changed_root).unwrap();
    let changed_config = write_loadable_config(&changed_root);
    registry
        .register(
            &changed_config.canonicalize().unwrap(),
            ServiceScope::User,
            SystemTime::now(),
        )
        .unwrap();
    fs::remove_file(&config).unwrap();
    fs::write(&changed_config, b"changed and still invalid").unwrap();
    let options = base_options(
        &temp,
        &binary_path,
        receipt_path,
        Some(target_version),
        false,
        false,
        "0.1.0",
    );
    let mut out = Vec::new();
    let error = run_upgrade(
        &options,
        &http,
        &registry,
        &FakeServiceManager::default(),
        &mut out,
    )
    .await
    .unwrap_err();
    assert_eq!(error.code(), ErrorCode::InstanceRegistryInvalid);
    assert!(out.is_empty());
    assert_eq!(fs::read(&binary_path).unwrap(), current);
}

#[tokio::test]
async fn upgrade_identifies_every_invalid_active_registration_before_replace() {
    let temp = TempDir::new().unwrap();
    let (binary_path, receipt_path, current) = write_upgradeable_pair(&temp, "0.1.0");
    let http = FixtureReleaseHttp::default();
    let target_version = env!("CARGO_PKG_VERSION");
    fixture_release(
        &http,
        "https://fixture.test/download",
        "https://fixture.test/api",
        target_version,
        host_target(),
        &fake_binary(target_version),
    );
    let registry = InstanceRegistry::with_roots(
        temp.path().join("registry/system"),
        temp.path().join("registry/user"),
    );
    let config_root = temp.path().join("invalid");
    fs::create_dir(&config_root).unwrap();
    let config = write_loadable_config(&config_root);
    registry
        .register(
            &config.canonicalize().unwrap(),
            ServiceScope::User,
            SystemTime::now(),
        )
        .unwrap();
    fs::write(&config, b"not valid toml").unwrap();
    let manager = FakeServiceManager::default();
    manager
        .install(ServiceScope::User, None, &binary_path)
        .unwrap();
    manager.start(ServiceScope::User).unwrap();
    let options = base_options(
        &temp,
        &binary_path,
        receipt_path,
        Some(target_version),
        false,
        false,
        "0.1.0",
    );
    let mut out = Vec::new();
    let error = run_upgrade(&options, &http, &registry, &manager, &mut out)
        .await
        .unwrap_err();
    assert_eq!(error.code(), ErrorCode::InstanceRegistryInvalid);
    assert!(out.is_empty());
    assert_eq!(fs::read(binary_path).unwrap(), current);
}
