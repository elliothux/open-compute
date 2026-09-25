use super::*;

#[tokio::test]
async fn resolve_latest_stable_tag_fail_closed_branches() {
    let http = FixtureReleaseHttp::default();
    let download = "https://fixture.test/releases/download";
    let latest = "https://fixture.test/releases/latest/download/release.json";

    http.insert(latest, b"not-json");
    assert_eq!(
        resolve_release(&http, download, None, host_target())
            .await
            .unwrap_err()
            .code(),
        ErrorCode::ReleaseUnsupported
    );

    http.insert(latest, r#"{"schemaVersion":1}"#);
    assert_eq!(
        resolve_release(&http, download, None, host_target())
            .await
            .unwrap_err()
            .code(),
        ErrorCode::ReleaseUnsupported
    );

    http.insert(
        latest,
        r#"{"schemaVersion":1,"tag":"v0.9.0","version":"0.8.0","gitRevision":"abc","workerdRelease":"1","workerdLockSha256":"cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd","artifacts":[]}"#,
    );
    assert!(
        resolve_release(&http, download, None, host_target())
            .await
            .unwrap_err()
            .message()
            .contains("inconsistent")
    );

    http.insert(
        latest,
        r#"{"schemaVersion":1,"tag":"v1.2.3-beta.1","version":"1.2.3-beta.1","gitRevision":"abc","workerdRelease":"1","workerdLockSha256":"cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd","artifacts":[]}"#,
    );
    assert!(
        resolve_release(&http, download, None, host_target())
            .await
            .unwrap_err()
            .message()
            .contains("stable SemVer")
    );
}
