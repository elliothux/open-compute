use super::*;

#[tokio::test]
async fn preflight_records_signed_http_and_skips_head_bucket() {
    let mock = MockS3::spawn("open-compute").await;
    let client = client_for(&mock).await;
    let out = preflight_object_storage(&client, PlatformId::generate(), StartupId::generate())
        .await
        .expect("preflight");
    assert_eq!(out.payload_bytes(), 32);
    assert_eq!(out.puts(), 1);
    assert_eq!(out.heads(), 2);
    assert_eq!(out.gets(), 1);
    assert_eq!(out.deletes(), 1);
    assert!(format!("{out:?}").contains("payload_bytes"));
    let canary = crate::PreflightOutcome::successful_canary();
    assert_eq!(canary, out);
    let rec = mock.recorded();
    assert!(rec.iter().all(|r| r.method != "HEAD"
        || r.path.contains("/preflight/")
        || r.path.contains("/artifacts/")
        || r.path.contains("/authority/")));
    assert!(
        !rec.iter().any(
            |r| r.method == "HEAD" && (r.path == "/open-compute" || r.path == "/open-compute/")
        )
    );
    let payload_ops: Vec<_> = rec
        .iter()
        .filter(|r| matches!(r.method.as_str(), "PUT" | "HEAD" | "GET" | "DELETE"))
        .collect();
    assert!(payload_ops.len() >= 5);
    assert!(payload_ops.iter().any(|r| r.method == "PUT"));
    assert!(payload_ops.iter().any(|r| r.method == "GET"));
    assert!(payload_ops.iter().all(|r| r.has_authorization));
    assert!(payload_ops.iter().all(|r| {
        r.authorization
            .as_deref()
            .is_some_and(|v| v.starts_with("AWS4-HMAC-SHA256 Credential="))
    }));
    assert_eq!(payload_ops[0].method, "GET");
    assert_eq!(mock.object_count(), 1);
}
