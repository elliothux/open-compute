use super::*;
use axum::extract::ws::WebSocketUpgrade;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn upgrade_request(uri: String) -> Request {
    Request::builder()
        .uri(uri)
        .header(header::HOST, "tenant.example")
        .header(header::CONNECTION, "Upgrade")
        .header(header::UPGRADE, "websocket")
        .header(header::SEC_WEBSOCKET_VERSION, "13")
        .header(header::SEC_WEBSOCKET_KEY, "AAECAwQFBgcICQoLDA0ODw==")
        .body(Body::empty())
        .unwrap()
}

#[test]
fn websocket_handshake_rejects_invalid_method_key_and_body() {
    for invalid in ["method", "key", "connection", "version", "body"] {
        let mut request = upgrade_request("http://tenant.example/".into());
        match invalid {
            "method" => *request.method_mut() = Method::POST,
            "key" => {
                request
                    .headers_mut()
                    .insert(header::SEC_WEBSOCKET_KEY, HeaderValue::from_static("bad"));
            }
            "connection" => {
                request.headers_mut().remove(header::CONNECTION);
            }
            "version" => {
                request.headers_mut().insert(
                    header::SEC_WEBSOCKET_VERSION,
                    HeaderValue::from_static("12"),
                );
            }
            "body" => {
                *request.body_mut() = Body::from("body");
            }
            _ => unreachable!(),
        }
        assert!(
            matches!(
                WebSocketHandshake::capture(&mut request),
                Err(StatusCode::BAD_REQUEST)
            ),
            "{invalid}"
        );
    }
    assert!(
        WebSocketHandshake::capture(&mut Request::new(Body::empty()))
            .unwrap()
            .is_none()
    );
    assert!(
        WebSocketHandshake::capture(&mut upgrade_request("http://tenant.example/".into()))
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn public_websocket_tunnels_frames_close_and_rejected_handshakes() {
    let backend = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let backend_port = backend.local_addr().unwrap().port();
    let backend_task = tokio::spawn(async move {
        axum::serve(
            backend,
            Router::new().fallback(|ws: WebSocketUpgrade, headers: HeaderMap| async move {
                assert_eq!(
                    headers.get("x-open-compute-original-method").unwrap(),
                    "GET"
                );
                assert!(headers.get("x-untrusted-hop").is_none());
                if headers.contains_key("x-reject") {
                    return StatusCode::FORBIDDEN.into_response();
                }
                ws.on_upgrade(|mut socket| async move {
                    while let Some(Ok(message)) = socket.recv().await {
                        if matches!(message, axum::extract::ws::Message::Close(_)) {
                            break;
                        }
                        if socket.send(message).await.is_err() {
                            break;
                        }
                    }
                })
            }),
        )
        .await
        .unwrap();
    });
    let auth = GenerationAuthRegistry::new();
    auth.activate_for_test(open_compute_core::SecretString::new("aa".repeat(32)));
    let transport = WorkerdTransport::for_test_endpoint(auth, backend_port);
    let target = DispatchTarget {
        account_id: AccountId::generate(),
        worker_id: WorkerId::generate(),
        version_id: VersionId::generate(),
        worker_code_sha256: "11".repeat(32),
        entrypoint: None,
        route_generation: 1,
        request_id: RequestId::generate(),
    };
    let public = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let public_port = public.local_addr().unwrap().port();
    let public_task = tokio::spawn(async move {
        axum::serve(
            public,
            Router::new().fallback(move |request: Request| {
                let transport = transport.clone();
                let target = target.clone();
                async move { transport.dispatch(target, request).await.unwrap() }
            }),
        )
        .await
        .unwrap();
    });
    let client: Client<HttpConnector, Body> = Client::builder(TokioExecutor::new()).build_http();
    let uri = format!("http://127.0.0.1:{public_port}/socket");
    let mut denied = upgrade_request(uri.clone());
    denied
        .headers_mut()
        .insert("x-reject", HeaderValue::from_static("1"));
    assert_eq!(
        client.request(denied).await.unwrap().status(),
        StatusCode::FORBIDDEN
    );
    let mut request = upgrade_request(uri);
    request.headers_mut().insert(
        header::CONNECTION,
        HeaderValue::from_static("Upgrade, x-untrusted-hop"),
    );
    request
        .headers_mut()
        .insert("x-untrusted-hop", HeaderValue::from_static("remove"));
    let mut response = client.request(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::SWITCHING_PROTOCOLS);
    assert_eq!(
        response.headers().get(header::UPGRADE).unwrap(),
        "websocket"
    );
    let mut socket = TokioIo::new(hyper::upgrade::on(&mut response).await.unwrap());
    for opcode in [0x81, 0x82] {
        socket
            .write_all(&[opcode, 0x82, 1, 2, 3, 4, b'h' ^ 1, b'i' ^ 2])
            .await
            .unwrap();
        let mut bytes = [0; 4];
        tokio::time::timeout(Duration::from_secs(2), socket.read_exact(&mut bytes))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(bytes, [opcode, 2, b'h', b'i']);
    }
    socket.write_all(&[0x88, 0x80, 1, 2, 3, 4]).await.unwrap();
    let mut rest = Vec::new();
    tokio::time::timeout(Duration::from_secs(2), socket.read_to_end(&mut rest))
        .await
        .unwrap()
        .unwrap();
    backend_task.abort();
    public_task.abort();
    let _ = backend_task.await;
    let _ = public_task.await;
}
