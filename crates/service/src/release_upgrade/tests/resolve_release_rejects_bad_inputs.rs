use super::*;

#[tokio::test]
async fn resolve_release_rejects_bad_inputs() {
    let http = FixtureReleaseHttp::default();
    let err = resolve_release(
        &http,
        "https://fixture.test/api",
        "https://fixture.test/download",
        Some("0.1.0-rc.1"),
        host_target(),
    )
    .await
    .unwrap_err();
    assert!(err.message().contains("stable SemVer"));
    let err = resolve_release(
        &http,
        "https://fixture.test/api",
        "https://fixture.test/download",
        Some("0.1.0"),
        "windows-x64",
    )
    .await
    .unwrap_err();
    assert_eq!(err.code(), ErrorCode::ReleaseUnsupported);
}
