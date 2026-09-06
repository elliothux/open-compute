use super::*;
use open_compute_core::SecretString;
use std::sync::atomic::{AtomicU16, Ordering};

#[tokio::test]
async fn revocation_batches_are_authenticated_and_unknown_results_fail_closed() {
    let calls = Arc::new(Mutex::new(Vec::<Vec<String>>::new()));
    let status = Arc::new(AtomicU16::new(204));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let app = Router::new().route(
        "/internal/worker-loaders/revoke",
        post({
            let calls = calls.clone();
            let status = status.clone();
            move |headers: HeaderMap, axum::Json(keys): axum::Json<Vec<String>>| {
                let calls = calls.clone();
                let status = status.clone();
                async move {
                    assert_eq!(headers[TOKEN_HEADER], "aa".repeat(32));
                    assert!(
                        headers[header::CONTENT_LENGTH]
                            .to_str()
                            .unwrap()
                            .parse::<usize>()
                            .unwrap()
                            <= 16384
                    );
                    assert!(keys.len() <= 128);
                    calls.lock().unwrap().push(keys);
                    StatusCode::from_u16(status.load(Ordering::SeqCst)).unwrap()
                }
            }
        }),
    );
    let (shutdown, receiver) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async {
        axum::serve(listener, app)
            .with_graceful_shutdown(async {
                let _ = receiver.await;
            })
            .await
            .unwrap();
    });
    let auth = GenerationAuthRegistry::new();
    let transport = WorkerdTransport::for_test_endpoint(auth.clone(), port);
    transport.revoke_worker_loaders(&[]).await.unwrap();
    assert!(
        transport
            .revoke_worker_loaders(&["a".repeat(64)])
            .await
            .is_err()
    );
    auth.activate_for_test(SecretString::new("aa".repeat(32)));
    let keys = (0..129)
        .map(|index| format!("{index:064x}"))
        .collect::<Vec<_>>();
    transport.revoke_worker_loaders(&keys).await.unwrap();
    assert_eq!(
        calls.lock().unwrap().as_slice(),
        &[keys[..128].to_vec(), keys[128..].to_vec()]
    );
    status.store(503, Ordering::SeqCst);
    let error = transport.revoke_worker_loaders(&keys).await.unwrap_err();
    assert_eq!(calls.lock().unwrap().len(), 3);
    assert!(!error.to_string().contains(&keys[0]));
    assert!(!error.to_string().contains("aa".repeat(32).as_str()));
    drop(transport);
    shutdown.send(()).unwrap();
    server.await.unwrap();
}
