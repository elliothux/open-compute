use super::*;

use std::sync::atomic::{AtomicU16, Ordering};

#[tokio::test]
async fn revocation_batches_are_bounded_and_unknown_results_fail_closed() {
    let calls = Arc::new(Mutex::new(Vec::<Vec<String>>::new()));
    let status = Arc::new(AtomicU16::new(204));
    let listener_dir = tempfile::tempdir_in("/tmp").unwrap();
    let listener_path = listener_dir.path().join("runtime.sock");
    let listener = tokio::net::UnixListener::bind(&listener_path).unwrap();
    let port = listener_path.clone();
    let app = Router::new().route(
        "/internal/worker-loaders/revoke",
        post({
            let calls = calls.clone();
            let status = status.clone();
            move |headers: HeaderMap, axum::Json(keys): axum::Json<Vec<String>>| {
                let calls = calls.clone();
                let status = status.clone();
                async move {
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

    let transport = WorkerdTransport::for_test_endpoint(&port);
    transport.revoke_worker_loaders(&[], None).await.unwrap();
    let inactive = WorkerdTransport::new(Arc::new(Mutex::new(None)));
    assert!(
        inactive
            .revoke_worker_loaders(&[format!("{}/", "a".repeat(64))], None)
            .await
            .is_err()
    );

    let keys = (0..129)
        .map(|index| format!("{index:064x}/"))
        .collect::<Vec<_>>();
    transport.revoke_worker_loaders(&keys, None).await.unwrap();
    assert_eq!(
        calls.lock().unwrap().as_slice(),
        &[keys[..128].to_vec(), keys[128..].to_vec()]
    );
    let error = transport
        .revoke_worker_loaders(&keys, Some(open_compute_core::StartupId::generate()))
        .await
        .unwrap_err();
    assert_eq!(error.code(), ErrorCode::RuntimeUnavailable);
    assert_eq!(calls.lock().unwrap().len(), 2);
    status.store(503, Ordering::SeqCst);
    let error = transport
        .revoke_worker_loaders(&keys, None)
        .await
        .unwrap_err();
    assert_eq!(calls.lock().unwrap().len(), 3);
    assert!(!error.to_string().contains(&keys[0]));
    assert!(!error.to_string().contains("aa".repeat(32).as_str()));
    drop(transport);
    shutdown.send(()).unwrap();
    server.await.unwrap();
}

#[tokio::test]
async fn revocation_transport_failures_classify_stable_evidence() {
    // A closed Unix socket reports the connect-failure evidence class without leaking keys.
    let listener_dir = tempfile::tempdir_in("/tmp").unwrap();
    let listener_path = listener_dir.path().join("runtime.sock");
    let listener = tokio::net::UnixListener::bind(&listener_path).unwrap();
    let port = listener_path.clone();
    drop(listener);

    let transport = WorkerdTransport::for_test_endpoint(&port);
    let keys = vec![format!("{}/", "c".repeat(64))];
    let error = transport
        .revoke_worker_loaders(&keys, None)
        .await
        .unwrap_err();
    assert!(!error.to_string().contains(&keys[0]));

    // An endpoint that accepts but never responds surfaces the bounded header-timeout class.
    let stalled_dir = tempfile::tempdir_in("/tmp").unwrap();
    let stalled_path = stalled_dir.path().join("runtime.sock");
    let stalled = tokio::net::UnixListener::bind(&stalled_path).unwrap();
    let stalled_port = stalled_path.clone();
    let holder = tokio::spawn(async move {
        loop {
            if let Ok((stream, _)) = stalled.accept().await {
                std::mem::forget(stream);
            }
        }
    });

    let transport = WorkerdTransport::for_test_endpoint(&stalled_port);
    let started = std::time::Instant::now();
    let error = transport
        .revoke_worker_loaders(&[format!("{}/", "d".repeat(64))], None)
        .await
        .unwrap_err();
    assert!(started.elapsed() < Duration::from_secs(60));
    assert!(!error.to_string().contains("cc".repeat(32).as_str()));
    holder.abort();
}
