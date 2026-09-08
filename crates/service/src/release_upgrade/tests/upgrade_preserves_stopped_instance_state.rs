use super::*;

#[tokio::test]
async fn upgrade_preserves_stopped_instance_state() {
    let temp = TempDir::new().unwrap();
    let (binary_path, receipt_path, _) = write_upgradeable_pair(&temp, "0.1.0");
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
    run_upgrade(&options, &http, &registry, &manager, &mut out)
        .await
        .unwrap();
    assert!(manager.started().is_empty());
    assert!(
        !String::from_utf8(out)
            .unwrap()
            .contains("UPGRADE_INSTANCE_RESTARTED")
    );
    assert_eq!(fs::read(&binary_path).unwrap(), next);
}
