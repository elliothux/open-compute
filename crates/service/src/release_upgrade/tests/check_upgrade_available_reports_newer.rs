use super::*;

#[tokio::test]
async fn check_upgrade_available_reports_newer() {
    let temp = TempDir::new().unwrap();
    let (binary_path, receipt_path, _) = write_upgradeable_pair(&temp, "0.1.0");
    let download_base = "https://fixture.test/download".to_owned();
    let api_base = "https://fixture.test/api".to_owned();
    let http = FixtureReleaseHttp::default();
    fixture_release(
        &http,
        &download_base,
        &api_base,
        "0.2.0",
        host_target(),
        &fake_binary("0.2.0"),
    );
    let result = check_upgrade_available(
        &http,
        &api_base,
        &download_base,
        "0.1.0",
        &receipt_path,
        &binary_path,
        host_target(),
    )
    .await
    .unwrap();
    assert_eq!(result.available_version.as_deref(), Some("0.2.0"));
    assert!(result.upgrade_allowed);
}
