use super::*;

#[tokio::test]
async fn dry_run_rejects_already_installed() {
    let temp = TempDir::new().unwrap();
    let (binary_path, receipt_path, _) = write_upgradeable_pair(&temp, "0.1.0");
    let download_base = "https://fixture.test/download".to_owned();
    let api_base = "https://fixture.test/api".to_owned();
    let http = FixtureReleaseHttp::default();
    fixture_release(
        &http,
        &download_base,
        &api_base,
        "0.1.0",
        host_target(),
        &fake_binary("0.1.0"),
    );
    let options = base_options(
        &temp,
        &binary_path,
        receipt_path,
        Some("0.1.0"),
        true,
        false,
        "0.1.0",
    );
    let err = run_upgrade(
        &options,
        &http,
        &InstanceRegistry::with_roots(
            temp.path().join("registry/system"),
            temp.path().join("registry/user"),
        ),
        &FakeServiceManager::default(),
        &mut Vec::new(),
    )
    .await
    .unwrap_err();
    assert!(err.message().contains("already installed"));
}
