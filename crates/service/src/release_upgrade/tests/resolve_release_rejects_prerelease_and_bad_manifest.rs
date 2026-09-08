use super::*;

#[tokio::test]
async fn resolve_release_rejects_prerelease_and_bad_manifest() {
    let http = FixtureReleaseHttp::default();
    let api = "https://fixture.test/api";
    http.insert(
        format!("{api}/repos/elliothux/open-compute/releases/latest"),
        r#"{"tag_name":"v0.2.0-rc.1","prerelease":true,"draft":false}"#,
    );
    let err = resolve_release(
        &http,
        api,
        "https://fixture.test/download",
        None,
        host_target(),
    )
    .await
    .unwrap_err();
    assert!(err.message().contains("prerelease") || err.message().contains("stable"));

    let download = "https://fixture.test/download";
    http.insert(format!("{download}/v0.2.0/release.json"), b"{not-json");
    http.insert(format!("{download}/v0.2.0/SHA256SUMS"), b"deadbeef  x\n");
    let err = resolve_release(&http, api, download, Some("0.2.0"), host_target())
        .await
        .unwrap_err();
    assert_eq!(err.code(), ErrorCode::ReleaseUnsupported);
}
