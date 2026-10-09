use super::*;
use axum::Router;
use axum::body::Body;
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use axum::routing::get;
use serde_json::json;
use tokio::net::TcpListener;

#[tokio::test]
async fn discovery_bounds_identity_credentials_redirects_and_response_bytes() {
    for scenario in [
        "valid",
        "host",
        "port",
        "scheme",
        "page",
        "redirect",
        "malformed",
        "declared",
        "chunked",
    ] {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let endpoint = match scenario {
            "host" => format!("ws://localhost:{}/devtools/browser/native", address.port()),
            "port" => "ws://127.0.0.1:1/devtools/browser/native".into(),
            "scheme" => format!("wss://{address}/devtools/browser/native"),
            "page" => format!("ws://{address}/devtools/page/native"),
            _ => format!("ws://{address}/devtools/browser/native"),
        };
        let expected = endpoint.clone();
        let app = Router::new().route(
            "/json/version",
            get(move |headers: HeaderMap| {
                let endpoint = endpoint.clone();
                async move {
                    assert_eq!(headers["authorization"], "Bearer fixture-credential");
                    let (status, body) = match scenario {
                        "redirect" => (StatusCode::FOUND, "redirect".into()),
                        "malformed" => (StatusCode::OK, "not JSON".into()),
                        "declared" | "chunked" => (StatusCode::OK, "x".repeat(65537)),
                        _ => (
                            StatusCode::OK,
                            json!({"webSocketDebuggerUrl":endpoint}).to_string(),
                        ),
                    };
                    let mut response = Response::builder().status(status);
                    if scenario == "redirect" {
                        response = response.header("location", "https://example.com/secret");
                    }
                    if scenario == "declared" {
                        response = response.header("content-length", "65537");
                    }
                    let body = if scenario == "chunked" {
                        Body::from_stream(futures::stream::iter([Ok::<_, std::io::Error>(body)]))
                    } else {
                        Body::from(body)
                    };
                    response.body(body).unwrap()
                }
            }),
        );
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let input = Url::parse(&format!("http://{address}")).unwrap();
        let result = discover(
            &input,
            Some("Bearer fixture-credential"),
            Duration::from_secs(1),
        )
        .await;
        if scenario == "valid" {
            assert_eq!(result.unwrap().as_str(), expected);
        } else {
            let error = result.unwrap_err();
            assert_eq!(error.code(), ErrorCode::BrowserUnavailable, "{scenario}");
            assert!(!error.to_string().contains("fixture-credential"));
        }
        task.abort();
        let _ = task.await;
    }
}

#[tokio::test]
async fn protocol_and_frontend_stay_on_the_configured_origin_with_bounded_bytes_and_paths() {
    let transport =
        OperatorHttpClient::new(open_compute_core::OperatorProxyPolicy::default()).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let app = Router::new()
        .route(
            "/json/protocol",
            get(|| async { axum::Json(json!({"domains":[{"domain":"Page"}]})) }),
        )
        .route(
            "/devtools/{*asset}",
            get(
                |axum::extract::Path(asset): axum::extract::Path<String>| async move {
                    match asset.as_str() {
                        "inspector.html" => Response::new(Body::from(
                            "<!doctype html><title>Chrome DevTools</title>",
                        )),
                        "redirect.js" => Response::builder()
                            .status(StatusCode::FOUND)
                            .header("location", "http://localhost/private")
                            .body(Body::empty())
                            .unwrap(),
                        "oversized.js" => Response::new(Body::from(vec![b'x'; 4097])),
                        _ => Response::new(Body::from("export {};")),
                    }
                },
            ),
        );
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    // HTTP discovery and explicit browser WS endpoints use the same native HTTP origin.
    for url in [
        format!("http://{address}/json/version"),
        format!("ws://{address}/devtools/browser/native"),
    ] {
        let config = super::super::tests::config(url);
        assert_eq!(
            BrowserBackend::protocol(&config).await.unwrap()["domains"][0]["domain"],
            "Page"
        );
        for (asset, media) in [
            ("inspector.html", "text/html; charset=utf-8"),
            ("entrypoints/main.js", "text/javascript; charset=utf-8"),
            ("style.css", "text/css; charset=utf-8"),
            ("font.woff2", "font/woff2"),
            ("image.png", "image/png"),
            ("image.svg", "image/svg+xml"),
            ("module.wasm", "application/wasm"),
            ("types.json", "application/json"),
            ("image.webp", "image/webp"),
            ("other.dat", "application/octet-stream"),
        ] {
            let (bytes, actual) = BrowserBackend::frontend(&config, &transport, asset)
                .await
                .unwrap();
            assert!(!bytes.is_empty());
            assert_eq!(actual, media);
        }
        for asset in [
            "../private",
            "/inspector.html",
            "entrypoints//main.js",
            "entrypoints/./main.js",
            "private%2Ffile",
            "foo?secret=1",
            "redirect.js",
            "oversized.js",
        ] {
            assert_eq!(
                BrowserBackend::frontend(&config, &transport, asset)
                    .await
                    .unwrap_err()
                    .code(),
                ErrorCode::BrowserUnavailable
            );
        }
    }
    task.abort();
}

#[tokio::test]
async fn frontend_resources_reuse_the_same_upstream_connection() {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let config = super::super::tests::config(format!(
        "ws://{}/devtools/browser/native",
        listener.local_addr().unwrap()
    ));
    let transport =
        OperatorHttpClient::new(open_compute_core::OperatorProxyPolicy::default()).unwrap();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        for path in ["first.js", "nested/second.js"] {
            let mut request = Vec::new();
            while !request.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
                assert!(request.len() < 4096);
                let mut chunk = [0; 512];
                let count = socket.read(&mut chunk).await.unwrap();
                assert_ne!(count, 0, "upstream connection closed between resources");
                request.extend_from_slice(&chunk[..count]);
            }
            assert!(request.starts_with(format!("GET /devtools/{path} HTTP/1.1\r\n").as_bytes()));
            socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
                .await
                .unwrap();
        }
    });
    tokio::time::timeout(Duration::from_secs(5), async {
        for asset in ["first.js", "nested/second.js"] {
            assert_eq!(
                BrowserBackend::frontend(&config, &transport, asset)
                    .await
                    .unwrap()
                    .0,
                b"ok"
            );
        }
        server.await.unwrap();
    })
    .await
    .unwrap();
}
