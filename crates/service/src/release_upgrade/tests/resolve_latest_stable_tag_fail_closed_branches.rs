use super::*;

#[tokio::test]
async fn resolve_latest_stable_tag_fail_closed_branches() {
    let http = FixtureReleaseHttp::default();
    let api = "https://fixture.test/api";
    let download = "https://fixture.test/download";
    let latest = format!("{api}/repos/elliothux/open-compute/releases/latest");

    http.insert(latest.clone(), b"not-json");
    assert_eq!(
        resolve_release(&http, api, download, None, host_target())
            .await
            .unwrap_err()
            .code(),
        ErrorCode::ReleaseUnsupported
    );

    http.insert(latest.clone(), r#"{"prerelease":false,"draft":false}"#);
    assert!(
        resolve_release(&http, api, download, None, host_target())
            .await
            .unwrap_err()
            .message()
            .contains("tag_name")
    );

    http.insert(
        latest.clone(),
        r#"{"tag_name":"v0.9.0","prerelease":false,"draft":true}"#,
    );
    let err = resolve_release(&http, api, download, None, host_target())
        .await
        .unwrap_err();
    assert!(
        err.message().contains("prerelease") || err.message().contains("draft"),
        "{err:?}"
    );

    // Passes GitHub draft/prerelease gates but fails stable SemVer / leading-v checks.
    http.insert(
        latest.clone(),
        r#"{"tag_name":"1.2.3","prerelease":false,"draft":false}"#,
    );
    assert!(
        resolve_release(&http, api, download, None, host_target())
            .await
            .unwrap_err()
            .message()
            .contains("stable SemVer")
    );
    http.insert(
        latest,
        r#"{"tag_name":"v1.2.3-beta.1","prerelease":false,"draft":false}"#,
    );
    assert!(
        resolve_release(&http, api, download, None, host_target())
            .await
            .unwrap_err()
            .message()
            .contains("stable SemVer")
    );
}
