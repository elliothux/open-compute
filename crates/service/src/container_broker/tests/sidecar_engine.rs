//! Authenticated sidecar protocol fixture.

use super::*;

pub(super) async fn sidecar(
    State(state): State<Arc<MockEngine>>,
    mut request: Request,
) -> Response<Body> {
    let credentials = state.credentials.lock().await;
    let authorized = credentials.as_ref().is_some_and(|credentials| {
        request
            .headers()
            .get("X-Open-Compute-Container-Control")
            .and_then(|v| v.to_str().ok())
            == Some(credentials.control_token.as_str())
    });
    if !authorized {
        drop(credentials);
        state
            .sidecar_auth_rejections
            .fetch_add(1, Ordering::Relaxed);
        let delay = *state.sidecar_auth_delay.lock().await;
        if let Some(delay) = delay {
            tokio::time::sleep(delay).await;
        }
        let callback_status = state.sidecar_callback_auth_status.load(Ordering::Relaxed);
        let status = if callback_status != 0
            && request.method() == Method::POST
            && request.uri().path() == "/open-compute/probe-callback"
        {
            callback_status
        } else {
            401
        };
        return Response::builder()
            .status(status)
            .body(Body::empty())
            .unwrap();
    }
    let callback_token = credentials.as_ref().unwrap().callback_token.clone();
    drop(credentials);
    let path = request.uri().path();
    if path == "/open-compute/callback-channel" {
        let upgrade = hyper::upgrade::on(&mut request);
        tokio::spawn(async move {
            *state.reverse.lock().await =
                Some(hyper_util::rt::TokioIo::new(upgrade.await.unwrap()));
        });
        return Response::new(Body::empty());
    }
    if path == "/open-compute/probe-callback" {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
        let mut reverse = loop {
            if let Some(reverse) = state.reverse.lock().await.take() {
                break reverse;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        };
        reverse.write_all(format!("CONNECT open-compute-probe.invalid:1 HTTP/1.1\r\nX-Proto: tcp\r\nX-Open-Compute-Container-Callback: {callback_token}\r\n\r\n").as_bytes()).await.unwrap();
        let mut bytes = Vec::new();
        loop {
            let byte = reverse.read_u8().await.unwrap();
            bytes.push(byte);
            if bytes.ends_with(b"\r\n\r\n") {
                break;
            }
        }
        assert!(bytes.starts_with(b"HTTP/1.1 202 "));
        return Response::builder().status(204).body(Body::empty()).unwrap();
    }
    if path == "/open-compute/profile" {
        let gate = state.sidecar_profile_gate.lock().await.clone();
        if let Some(gate) = gate {
            gate.notified().await;
        }
        if let Some((status, body)) = state.sidecar_profile_override.lock().await.clone() {
            return Response::builder()
                .status(status)
                .header("Content-Type", "application/json")
                .body(Body::from(body))
                .unwrap();
        }
        Response::builder()
            .header("Content-Type", "application/json")
            .body(Body::from(
                r#"{"protocol":"open-compute-container-sidecar","version":2}"#,
            ))
            .unwrap()
    } else if path == "/ca" {
        Response::new(Body::from(include_str!("fixtures/test-ca.pem")))
    } else {
        Response::builder().status(204).body(Body::empty()).unwrap()
    }
}
