use super::*;

#[tokio::test]
async fn upgrade_restart_failure_stops_remaining() {
    let temp = TempDir::new().unwrap();
    let (binary_path, receipt_path, _) = write_upgradeable_pair(&temp, "0.1.0");
    let download_base = "https://fixture.test/download".to_owned();
    let api_base = "https://fixture.test/api".to_owned();
    let http = FixtureReleaseHttp::default();
    let next = fake_binary("0.1.4");
    fixture_release(
        &http,
        &download_base,
        &api_base,
        "0.1.4",
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
    manager.set_fail_restart(true);
    let options = base_options(
        &temp,
        &binary_path,
        receipt_path,
        Some("0.1.4"),
        false,
        false,
        "0.1.0",
    );
    let mut out = Vec::new();
    let err = run_upgrade(&options, &http, &registry, &manager, &mut out)
        .await
        .unwrap_err();
    assert_eq!(err.code(), ErrorCode::PlatformUnavailable);
    assert!(
        String::from_utf8(out)
            .unwrap()
            .contains("UPGRADE_INSTANCE_FAILED")
    );
    // Binary already replaced before restart.
    assert_eq!(fs::read(&binary_path).unwrap(), next);
}
