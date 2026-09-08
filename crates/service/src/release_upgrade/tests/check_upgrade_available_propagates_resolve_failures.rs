use super::*;

#[tokio::test]
async fn check_upgrade_available_propagates_resolve_failures() {
    let temp = TempDir::new().unwrap();
    let binary = temp.path().join("bin/ocd");
    fs::create_dir_all(binary.parent().unwrap()).unwrap();
    fs::write(&binary, b"ocd").unwrap();
    let http = FixtureReleaseHttp::default();
    let err = check_upgrade_available(
        &http,
        "https://fixture.test/api",
        "https://fixture.test/download",
        "0.1.0",
        &temp.path().join("missing-receipt.json"),
        &binary,
        host_target(),
    )
    .await
    .unwrap_err();
    assert_eq!(err.code(), ErrorCode::PlatformUnavailable);
}
