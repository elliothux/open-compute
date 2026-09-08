use super::*;

#[tokio::test]
async fn preflight_timeout_is_secret_safe() {
    let mock = MockS3::spawn("open-compute").await;
    mock.set_fault(Fault::Timeout);
    let mut cfg = s3_config(&mock.endpoint);
    cfg.request_timeout_ms = 200;
    cfg.connect_timeout_ms = 100;
    cfg.max_retries = 1;
    let creds = resolve_s3_credentials_with(&cfg, &env()).unwrap();
    let client = ObjectBackend::connect_s3(&cfg, &creds, 1024).unwrap();
    let err = preflight_object_storage(&client, PlatformId::generate(), StartupId::generate())
        .await
        .unwrap_err();
    assert_eq!(err.code(), ErrorCode::ObjectStorageUnavailable);
    let json = serde_json::to_string(&err).unwrap();
    assert!(!json.contains("AKIA"));
    assert!(!json.contains("AWS4"));
}
