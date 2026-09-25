//! Real-process stale Provider ACK isolation across two instances.

use super::*;

pub(super) async fn assert_stale_provider_ack_is_scoped(
    address: SocketAddr,
    alpha: &str,
    beta: &str,
    data_a: &Path,
    data_b: &Path,
) {
    let work_a = data_a.join("runtime/extensions/local-files");
    let work_b = data_b.join("runtime/extensions/local-files");
    let before_a: serde_json::Value =
        serde_json::from_slice(&fs::read(work_a.join("provider.lease")).unwrap()).unwrap();
    let before_b = fs::read(work_b.join("provider.lease")).unwrap();
    let marker = work_a.join(".ocd-test-duplicate-ack-once");
    fs::write(&marker, []).unwrap();
    fs::set_permissions(&marker, fs::Permissions::from_mode(0o600)).unwrap();
    let host_a = format!("ack-worker.{alpha}.localhost");
    let host_b = format!("shared-worker.{beta}.localhost");
    upload_ack_worker(address, alpha, "first").await;
    let (status, body) = request_http(address, "GET", &host_a, "alpha-deployer", "/").await;
    assert_eq!(status, 200, "A first attach: {body}");
    assert!(!marker.exists());
    upload_ack_worker(address, alpha, "second").await;
    let (status, _) = request_http(address, "GET", &host_a, "alpha-deployer", "/").await;
    assert_ne!(status, 200, "stale ACK authorized a new A session");
    let (status, body) = request_http(address, "GET", &host_b, "beta-deployer", "/provider").await;
    assert_eq!(status, 200, "B affected by A stale ACK: {body}");
    assert_eq!(fs::read(work_b.join("provider.lease")).unwrap(), before_b);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    loop {
        let (status, body) = request_http(address, "GET", &host_a, "alpha-deployer", "/").await;
        if status == 200 {
            assert!(body.contains("alpha-provider"));
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "A Provider did not recover after stale ACK"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let after_a: serde_json::Value =
        serde_json::from_slice(&fs::read(work_a.join("provider.lease")).unwrap()).unwrap();
    assert_ne!(after_a["pid"], before_a["pid"]);
}

async fn upload_ack_worker(address: SocketAddr, instance_id: &str, revision: &str) {
    let boundary = "ocd-r1-ack-worker-upload";
    let metadata = r#"{"main_module":"index.js","compatibility_date":"2026-09-08","bindings":[{"name":"FILES","type":"service","service":"local-files","props":{"directory":"workspace"}}]}"#;
    let source = format!(
        "export default {{ async fetch(_request, env) {{ return new Response('{revision}:' + await env.FILES.read('owner.txt')); }} }};"
    );
    let body = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"metadata\"\r\nContent-Type: application/json\r\n\r\n{metadata}\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"index.js\"; filename=\"index.js\"\r\nContent-Type: application/javascript+module\r\n\r\n{source}\r\n--{boundary}--\r\n"
    );
    let (status, response) = request_http_body_with_headers(
        address,
        "PUT",
        "127.0.0.1",
        "alpha-deployer",
        &format!("/client/v4/accounts/{instance_id}/workers/scripts/ack-worker"),
        &body,
        &format!("Content-Type: multipart/form-data; boundary={boundary}\r\n"),
    )
    .await;
    assert_eq!(status, 200, "{response}");
}
