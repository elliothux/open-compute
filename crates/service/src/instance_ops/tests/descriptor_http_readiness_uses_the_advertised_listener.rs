use super::*;

#[test]
fn descriptor_http_readiness_uses_the_advertised_listener() {
    let (listener, server) = one_shot_health_response("200 OK");
    let ready = descriptor_http_ready(&http_descriptor(Some(listener), "ready"));
    let request = server.join().unwrap();
    assert!(ready, "request was {request:?}");
    assert!(request.starts_with("GET /health/ready "));

    let (listener, server) = one_shot_health_response("503 Service Unavailable");
    assert!(!descriptor_http_ready(&http_descriptor(
        Some(listener),
        "ready"
    )));
    let _ = server.join().unwrap();

    assert!(!descriptor_http_ready(&http_descriptor(None, "ready")));
    assert!(!descriptor_http_ready(&http_descriptor(
        Some("not-an-address".to_owned()),
        "ready"
    )));
    assert!(!descriptor_http_ready(&http_descriptor(
        Some("127.0.0.1:1".to_owned()),
        "ready"
    )));
}
