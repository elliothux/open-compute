use super::*;

#[tokio::test]
async fn upgrade_reports_and_skips_stale_stopped_registration() {
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
    let missing_record = registry
        .register_owned(
            &config.canonicalize().unwrap(),
            &binary_path,
            ServiceScope::User,
            None,
            SystemTime::now(),
        )
        .unwrap();
    fs::remove_file(&config).unwrap();
    let changed_root = temp.path().join("changed");
    fs::create_dir(&changed_root).unwrap();
    let changed_config = write_loadable_config(&changed_root);
    let changed_record = registry
        .register_owned(
            &changed_config.canonicalize().unwrap(),
            &binary_path,
            ServiceScope::User,
            None,
            SystemTime::now(),
        )
        .unwrap();
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
    run_upgrade(
        &options,
        &http,
        &registry,
        &FakeServiceManager::default(),
        &mut out,
    )
    .await
    .unwrap();
    let output = String::from_utf8(out).unwrap();
    assert!(output.contains("UPGRADE_STALE_INSTANCE"));
    assert_eq!(output.matches("UPGRADE_STALE_INSTANCE").count(), 2);
    assert!(output.contains(&changed_record.instance_id));
    assert!(output.contains(&format!(
        "ocd instance unregister --instance {}",
        missing_record.instance_id
    )));
    assert_ne!(fs::read(&binary_path).unwrap(), current);
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
    let record = registry
        .register_owned(
            &config.canonicalize().unwrap(),
            &binary_path,
            ServiceScope::User,
            None,
            SystemTime::now(),
        )
        .unwrap();
    fs::write(&config, b"not valid toml").unwrap();
    let manager = FakeServiceManager::default();
    manager.install(&record, &binary_path).unwrap();
    manager.start(&record).unwrap();
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
    let output = String::from_utf8(out).unwrap();
    assert!(output.contains(&record.instance_id));
    assert!(output.contains(config.to_string_lossy().as_ref()));
    assert!(output.contains("instance unregister"));
    assert_eq!(fs::read(binary_path).unwrap(), current);
}
