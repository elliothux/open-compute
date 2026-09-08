use super::*;

#[tokio::test]
async fn fixture_http_enforces_size_bound_and_missing_url() {
    let http = FixtureReleaseHttp::default();
    http.insert("https://fixture.test/big", vec![0u8; 8]);
    let err = http.get("https://fixture.test/big", 4).await.unwrap_err();
    assert_eq!(err.code(), ErrorCode::LimitInvalid);
    let err = http
        .get("https://fixture.test/missing", 1024)
        .await
        .unwrap_err();
    assert_eq!(err.code(), ErrorCode::PlatformUnavailable);
}
