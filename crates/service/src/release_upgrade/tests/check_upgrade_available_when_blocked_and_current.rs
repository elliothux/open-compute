use super::*;

#[tokio::test]
async fn check_upgrade_available_when_blocked_and_current() {
    let temp = TempDir::new().unwrap();
    let binary = temp.path().join("bin/ocd");
    fs::create_dir_all(binary.parent().unwrap()).unwrap();
    fs::write(&binary, b"ocd").unwrap();
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
    let result = check_upgrade_available(
        &http,
        &api_base,
        &download_base,
        "0.1.0",
        &temp.path().join("missing-receipt.json"),
        &binary,
        host_target(),
    )
    .await
    .unwrap();
    assert!(!result.upgrade_allowed);
    assert!(result.available_version.is_none());
}
