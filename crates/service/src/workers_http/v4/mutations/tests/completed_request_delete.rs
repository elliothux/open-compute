use super::*;
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[tokio::test]
async fn completed_request_releases_its_version_and_script_delete_drains_promptly() {
    let (_temp, _mock, state, account, storage) =
        crate::tests::initialized_worker_http_fixture().await;
    let seeded = seed_script_versions(&storage, account);
    let sockets = tempfile::tempdir_in("/tmp").unwrap();
    let socket = sockets.path().join("runtime.sock");
    let listener = tokio::net::UnixListener::bind(&socket).unwrap();
    let (finish, finished) = tokio::sync::oneshot::channel::<()>();
    let finished = Arc::new(Mutex::new(Some(finished)));
    let runtime = Router::new()
        .route(
            "/internal/dispatch",
            axum::routing::post(move || {
                let finished = finished.lock().unwrap().take().unwrap();
                async move {
                    let body = futures::stream::once(async move {
                        finished.await.map_err(std::io::Error::other)?;
                        Ok::<_, std::io::Error>("done")
                    });
                    (
                        [("x-open-compute-execution-started", "1")],
                        Body::from_stream(body),
                    )
                }
            }),
        )
        .route(
            "/internal/worker-loaders/revoke",
            axum::routing::post(|| async { StatusCode::NO_CONTENT }),
        );
    let (stop, stopped) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        axum::serve(listener, runtime)
            .with_graceful_shutdown(async {
                let _ = stopped.await;
            })
            .await
            .unwrap();
    });
    let mut api = state.worker_api().unwrap().as_ref().clone();
    let pins = api.pins.clone();
    api.transport = crate::runtime_bridge::WorkerdTransport::for_test_endpoint(&socket)
        .with_version_pins(pins.clone());
    let authority = crate::cloudflare_v4::accounts::V4InstanceContext::new(account, 1);
    let prefix = format!(
        "/client/v4/accounts/{}/workers/scripts/settings-worker",
        authority.public_id()
    );
    let state = state
        .with_worker_api(api)
        .with_platform_storage(storage.clone())
        .with_v4_tokens(
            SecretString::new("deployer-token"),
            SecretString::new("read-token"),
        )
        .with_v4_instance_context(authority);
    let public = crate::http::public_router(state.clone());
    let admin = crate::http::admin_router(state);
    let response = public
        .oneshot(
            Request::builder()
                .uri("/x")
                .header(header::HOST, format!("settings-worker.{account}.localhost"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(pins.count(seeded.version) > 0);
    let blocked = admin
        .clone()
        .oneshot(request(Method::DELETE, &prefix, Body::empty()))
        .await
        .unwrap();
    assert_eq!(
        blocked.status(),
        StatusCode::CONFLICT,
        "a live response must block deletion"
    );
    finish.send(()).unwrap();
    assert_eq!(to_bytes(response.into_body(), 16).await.unwrap(), "done");
    let deleted = tokio::time::timeout(
        Duration::from_secs(1),
        admin.oneshot(request(Method::DELETE, &prefix, Body::empty())),
    )
    .await
    .expect("completed responses must not consume the delete drain timeout")
    .unwrap();
    assert_eq!(
        deleted.status(),
        StatusCode::OK,
        "completed request left {} pins",
        pins.count(seeded.version)
    );
    assert_eq!(pins.count(seeded.version), 0);
    assert!(
        WorkerRepository::new(storage.db())
            .get_worker(account, seeded.worker.id)
            .unwrap()
            .deleted_at_ms
            .is_some()
    );
    stop.send(()).unwrap();
    server.await.unwrap();
}
