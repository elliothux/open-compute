use super::*;

#[tokio::test]
async fn resolve_release_rejects_prerelease_and_bad_manifest() {
    let http = FixtureReleaseHttp::default();
    http.insert(
        "https://fixture.test/latest/download/release.json",
        r#"{"schemaVersion":1,"tag":"v0.2.0-rc.1","version":"0.2.0-rc.1","gitRevision":"abc","workerdRelease":"1","workerdLockSha256":"cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd","artifacts":[]}"#,
    );
    let err = resolve_release(&http, "https://fixture.test/download", None, host_target())
        .await
        .unwrap_err();
    assert!(err.message().contains("stable"));

    let download = "https://fixture.test/download";
    http.insert(format!("{download}/v0.2.0/release.json"), b"{not-json");
    http.insert(format!("{download}/v0.2.0/SHA256SUMS"), b"deadbeef  x\n");
    let err = resolve_release(&http, download, Some("0.2.0"), host_target())
        .await
        .unwrap_err();
    assert_eq!(err.code(), ErrorCode::ReleaseUnsupported);
}
