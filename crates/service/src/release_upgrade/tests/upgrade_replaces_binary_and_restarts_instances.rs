use super::*;

#[tokio::test]
async fn upgrade_replaces_binary_and_restarts_instances() {
    let temp = TempDir::new().unwrap();
    let (binary_path, receipt_path, current) = write_upgradeable_pair(&temp, "0.1.0");
    let download_base = "https://fixture.test/download".to_owned();
    let api_base = "https://fixture.test/api".to_owned();
    let http = FixtureReleaseHttp::default();
    let target_version = env!("CARGO_PKG_VERSION");
    let next = fake_binary(target_version);
    fixture_release(
        &http,
        &download_base,
        &api_base,
        target_version,
        host_target(),
        &next,
    );
    let registry = InstanceRegistry::with_roots(
        temp.path().join("registry/system"),
        temp.path().join("registry/user"),
    );
    let config = write_loadable_config(temp.path());
    let record = registry
        .register(
            &config.canonicalize().unwrap(),
            ServiceScope::User,
            SystemTime::now(),
        )
        .unwrap();
    let manager = FakeServiceManager::default();
    manager.install(&record, &binary_path).unwrap();
    manager.start(&record).unwrap();
    let options = base_options(
        &temp,
        &binary_path,
        receipt_path.clone(),
        None,
        false,
        false,
        "0.1.0",
    );
    let mut out = Vec::new();
    run_upgrade(&options, &http, &registry, &manager, &mut out)
        .await
        .unwrap();
    let text = String::from_utf8(out).unwrap();
    assert!(text.contains(&format!("UPGRADE_OK {target_version}")));
    assert!(text.contains("UPGRADE_INSTANCE_RESTARTED"));
    assert_eq!(fs::read(&binary_path).unwrap(), next);
    assert_ne!(fs::read(&binary_path).unwrap(), current);
    let receipt = read_receipt(&receipt_path).unwrap();
    assert_eq!(receipt.version, target_version);
}
