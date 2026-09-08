use super::*;
use hyper::body::Incoming as HyperIncoming;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request as HyperRequest, Response, StatusCode};
use hyper_util::rt::TokioIo;
use std::convert::Infallible;
use tokio::net::TcpListener;

async fn serve_response(status: StatusCode, body: Vec<u8>, delay: Option<Duration>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        if let Some(delay) = delay {
            tokio::time::sleep(delay).await;
        }
        let io = TokioIo::new(stream);
        let body = body.clone();
        let _ = http1::Builder::new()
            .serve_connection(
                io,
                service_fn(move |_req: HyperRequest<HyperIncoming>| {
                    let body = body.clone();
                    async move {
                        Ok::<_, Infallible>(
                            Response::builder()
                                .status(status)
                                .body(Full::new(Bytes::from(body)))
                                .unwrap(),
                        )
                    }
                }),
            )
            .await;
    });
    format!("http://{addr}/release.bin")
}

async fn serve_redirect(location: String) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let io = TokioIo::new(stream);
        let _ = http1::Builder::new()
            .serve_connection(
                io,
                service_fn(move |_req: HyperRequest<HyperIncoming>| {
                    let location = location.clone();
                    async move {
                        Ok::<_, Infallible>(
                            Response::builder()
                                .status(StatusCode::FOUND)
                                .header(hyper::header::LOCATION, location)
                                .body(Full::new(Bytes::new()))
                                .unwrap(),
                        )
                    }
                }),
            )
            .await;
    });
    format!("http://{addr}/release.bin")
}

async fn serve_repeated_redirect(location: Option<String>, requests: usize) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let url = format!("http://{addr}/release.bin");
    let location = location.unwrap_or_else(|| url.clone());
    tokio::spawn(async move {
        for _ in 0..requests {
            let (stream, _) = listener.accept().await.unwrap();
            let io = TokioIo::new(stream);
            let location = location.clone();
            let _ = http1::Builder::new()
                .serve_connection(
                    io,
                    service_fn(move |_req: HyperRequest<HyperIncoming>| {
                        let location = location.clone();
                        async move {
                            Ok::<_, Infallible>(
                                Response::builder()
                                    .status(StatusCode::FOUND)
                                    .header(hyper::header::LOCATION, location)
                                    .body(Full::new(Bytes::new()))
                                    .unwrap(),
                            )
                        }
                    }),
                )
                .await;
        }
    });
    url
}

async fn serve_redirect_without_location() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let io = TokioIo::new(stream);
        let _ = http1::Builder::new()
            .serve_connection(
                io,
                service_fn(|_req: HyperRequest<HyperIncoming>| async move {
                    Ok::<_, Infallible>(
                        Response::builder()
                            .status(StatusCode::FOUND)
                            .body(Full::new(Bytes::new()))
                            .unwrap(),
                    )
                }),
            )
            .await;
    });
    format!("http://{addr}/release.bin")
}

#[tokio::test]
async fn live_get_success_and_error_paths() {
    let client = LiveReleaseHttp::with_timeout(Duration::from_secs(2)).unwrap();
    let url = serve_response(StatusCode::OK, b"ok-bytes".to_vec(), None).await;
    let body = client.get(&url, 64).await.unwrap();
    assert_eq!(body, b"ok-bytes");

    let url = serve_response(StatusCode::NOT_FOUND, b"missing".to_vec(), None).await;
    let err = client.get(&url, 64).await.unwrap_err();
    assert_eq!(err.code(), ErrorCode::PlatformUnavailable);

    let url = serve_response(StatusCode::OK, vec![b'x'; 32], None).await;
    let err = client.get(&url, 8).await.unwrap_err();
    assert_eq!(err.code(), ErrorCode::LimitInvalid);

    let err = client.get("not a url", 8).await.unwrap_err();
    assert_eq!(err.code(), ErrorCode::ReleaseUnsupported);

    let err = client
        .get("http://127.0.0.1:1/no-listener", 8)
        .await
        .unwrap_err();
    assert_eq!(err.code(), ErrorCode::PlatformUnavailable);
}

#[tokio::test]
async fn live_get_times_out_when_server_stalls() {
    let client = LiveReleaseHttp::with_timeout(Duration::from_millis(40)).unwrap();
    let url = serve_response(
        StatusCode::OK,
        b"late".to_vec(),
        Some(Duration::from_millis(400)),
    )
    .await;
    let err = client.get(&url, 64).await.unwrap_err();
    assert_eq!(err.code(), ErrorCode::PlatformUnavailable);
    assert!(err.message().contains("timed out"));
}

#[tokio::test]
async fn live_get_follows_release_redirect() {
    let client = LiveReleaseHttp::with_timeout(Duration::from_secs(2)).unwrap();
    let final_url = serve_response(StatusCode::OK, b"release-bytes".to_vec(), None).await;
    let redirect_url = serve_redirect(final_url).await;
    assert_eq!(
        client.get(&redirect_url, 64).await.unwrap(),
        b"release-bytes"
    );
}

#[tokio::test]
async fn live_get_rejects_malformed_and_unbounded_redirects() {
    let client = LiveReleaseHttp::new().unwrap();

    let missing = serve_redirect_without_location().await;
    let err = client.get(&missing, 64).await.unwrap_err();
    assert_eq!(err.code(), ErrorCode::PlatformUnavailable);
    assert!(err.message().contains("no valid location"));

    let unsupported =
        serve_repeated_redirect(Some("ftp://example.test/release".to_owned()), 1).await;
    let err = client.get(&unsupported, 64).await.unwrap_err();
    assert_eq!(err.code(), ErrorCode::ReleaseUnsupported);
    assert!(err.message().contains("HTTP or HTTPS"));

    let invalid = serve_repeated_redirect(Some("http://[".to_owned()), 1).await;
    let err = client.get(&invalid, 64).await.unwrap_err();
    assert_eq!(err.code(), ErrorCode::ReleaseUnsupported);
    assert!(err.message().contains("location is invalid"));

    let loop_url = serve_repeated_redirect(None, MAX_REDIRECTS + 1).await;
    let err = client.get(&loop_url, 64).await.unwrap_err();
    assert_eq!(err.code(), ErrorCode::PlatformUnavailable);
    assert!(err.message().contains("redirect limit"));
}

#[tokio::test]
async fn fixture_http_covers_miss_and_size_bound() {
    let http = FixtureReleaseHttp::default();
    http.insert("http://example.test/a", b"abcdef");
    assert_eq!(
        http.get("http://example.test/a", 16).await.unwrap(),
        b"abcdef"
    );
    let err = http
        .get("http://example.test/missing", 16)
        .await
        .unwrap_err();
    assert_eq!(err.code(), ErrorCode::PlatformUnavailable);
    let err = http.get("http://example.test/a", 2).await.unwrap_err();
    assert_eq!(err.code(), ErrorCode::LimitInvalid);
}
