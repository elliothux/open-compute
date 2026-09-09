use super::*;

#[tokio::test]
async fn upgrade_validates_registered_configs_before_binary_replace() {
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
    let config = temp.path().join("compute.toml");
    fs::write(&config, b"not valid toml").unwrap();
    registry
        .register(
            &config.canonicalize().unwrap(),
            ServiceScope::User,
            SystemTime::now(),
        )
        .unwrap();
    let options = base_options(
        &temp,
        &binary_path,
        receipt_path,
        Some(target_version),
        false,
        false,
        "0.1.0",
    );
    assert!(
        run_upgrade(
            &options,
            &http,
            &registry,
            &FakeServiceManager::default(),
            &mut Vec::new(),
        )
        .await
        .is_err()
    );
    assert_eq!(fs::read(&binary_path).unwrap(), current);
}
