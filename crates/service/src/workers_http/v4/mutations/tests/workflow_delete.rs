use super::*;
use crate::runtime_bridge::{WorkerdTransport, WorkflowRunRequest};
use open_compute_core::{WorkflowFence, WorkflowInstanceId, WorkflowToken};
use open_compute_storage::workflows::WorkflowRepository;
use std::sync::Arc;
use std::time::Duration;

#[tokio::test]
async fn drained_workflow_allows_worker_delete_and_leaves_a_deletable_inactive_definition() {
    let (_temp, _mock, state, account, storage) =
        crate::tests::initialized_worker_http_fixture().await;
    let seeded = seed_script_versions(&storage, account);
    let repository = WorkflowRepository::new(storage.db());
    let definition = repository
        .create_definition(account, "delete-flow", 1)
        .unwrap();
    let version = repository
        .stage_version(account, definition.id, seeded.version, "Flow", 2)
        .unwrap();
    repository
        .finish_version(account, version.target.workflow_version_id, true, 3)
        .unwrap();
    let sockets = tempfile::tempdir_in("/tmp").unwrap();
    let socket = sockets.path().join("runtime.sock");
    let listener = tokio::net::UnixListener::bind(&socket).unwrap();
    let entered = Arc::new(tokio::sync::Notify::new());
    let finish = Arc::new(tokio::sync::Notify::new());
    let runtime = Router::new().route("/internal/workflow", axum::routing::post({
        let entered = entered.clone(); let finish = finish.clone();
        move |_: axum::body::Bytes| { let entered = entered.clone(); let finish = finish.clone(); async move {
            entered.notify_one(); finish.notified().await;
            axum::Json(serde_json::json!({"result":{"outcome":"complete","finalOrdinal":0,"outputBase64":"T0NEVgECAA=="},"loaderOutcome":"warm","drainIncomplete":false}))
        }}
    })).route("/internal/worker-loaders/revoke", axum::routing::post(|| async { StatusCode::NO_CONTENT }));
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
    let transport = WorkerdTransport::for_test_endpoint(&socket).with_version_pins(pins.clone());
    api.transport = transport.clone();
    let scheduler = Arc::new(
        open_compute_storage::scheduler::SchedulerStore::open(
            &storage.data_dir().ensure_scheduler_db().unwrap(),
            5000,
            0,
            account,
        )
        .unwrap(),
    );
    let workflow_api = crate::workflow_http::WorkflowApiState::new(
        storage.clone(),
        scheduler,
        transport.clone(),
        Default::default(),
    );
    let authority = crate::cloudflare_v4::accounts::V4InstanceContext::new(account, 1);
    let prefix = format!("/client/v4/accounts/{}", authority.public_id());
    let state = state
        .with_worker_api(api)
        .with_workflow_api(Some(workflow_api))
        .with_platform_storage(storage.clone())
        .with_v4_tokens(
            SecretString::new("deployer-token"),
            SecretString::new("read-token"),
        )
        .with_v4_instance_context(authority);
    let admin = crate::http::admin_router(state);
    let dispatch = tokio::spawn(async move {
        transport
            .dispatch_workflow(
                &version.target,
                &WorkflowRunRequest {
                    fence: WorkflowFence {
                        instance_id: WorkflowInstanceId::generate(),
                        instance_generation: 1,
                        run_token: WorkflowToken::from_bytes([3; 32]),
                    },
                    external_instance_id: "completed".to_owned(),
                    definition_name: "delete-flow".to_owned(),
                    created_at_ms: 0,
                    payload_base64: "T0NEVgECAA==".to_owned(),
                    rollback: false,
                    schedule: None,
                },
                Duration::from_secs(5),
            )
            .await
    });
    entered.notified().await;
    let worker_path = format!("{prefix}/workers/workers/settings-worker");
    assert_eq!(
        admin
            .clone()
            .oneshot(request(Method::DELETE, &worker_path, Body::empty()))
            .await
            .unwrap()
            .status(),
        StatusCode::CONFLICT
    );
    finish.notify_one();
    dispatch.await.unwrap().unwrap();
    assert_eq!(pins.count(seeded.version), 0);
    let deleted = tokio::time::timeout(
        Duration::from_secs(1),
        admin
            .clone()
            .oneshot(request(Method::DELETE, &worker_path, Body::empty())),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(deleted.status(), StatusCode::OK);
    repository.verify_catalog().unwrap();
    storage
        .db()
        .migrate(&open_compute_core::SystemClock)
        .unwrap();
    let workflow_path = format!("{prefix}/workflows/delete-flow");
    let detached = admin
        .clone()
        .oneshot(request(Method::GET, &workflow_path, Body::empty()))
        .await
        .unwrap();
    assert_eq!(detached.status(), StatusCode::OK);
    assert_eq!(
        response_json(detached).await["result"]["script_deleted"],
        true
    );
    assert_eq!(
        admin
            .clone()
            .oneshot(request(Method::DELETE, &workflow_path, Body::empty()))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        admin
            .oneshot(request(Method::GET, &workflow_path, Body::empty()))
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
    repository.verify_catalog().unwrap();
    stop.send(()).unwrap();
    server.await.unwrap();
}
