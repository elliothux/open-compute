use super::*;

#[tokio::test]
async fn upgrade_no_restart_skips_manager() {
    let temp = TempDir::new().unwrap();
    let (binary_path, receipt_path, _) = write_upgradeable_pair(&temp, "0.1.0");
    let download_base = "https://fixture.test/download".to_owned();
    let api_base = "https://fixture.test/api".to_owned();
    let http = FixtureReleaseHttp::default();
    let next = fake_binary("0.1.3");
    fixture_release(
        &http,
        &download_base,
        &api_base,
        "0.1.3",
        host_target(),
        &next,
    );
    let registry = InstanceRegistry::with_roots(
        temp.path().join("registry/system"),
        temp.path().join("registry/user"),
    );
    let manager = FakeServiceManager::default();
    let options = base_options(
        &temp,
        &binary_path,
        receipt_path,
        Some("0.1.3"),
        false,
        true,
        "0.1.0",
    );
    let mut out = Vec::new();
    run_upgrade(&options, &http, &registry, &manager, &mut out)
        .await
        .unwrap();
    assert!(String::from_utf8(out).unwrap().contains("--no-restart"));
    assert_eq!(fs::read(&binary_path).unwrap(), next);
    assert!(manager.started().is_empty());
}
