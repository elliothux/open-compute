//! Real pinned-workerd P0.2 dynamic Worker data-plane gate.

use axum::body::{Body, to_bytes};
use axum::http::{Request, header};
use base64::Engine as _;
use bytes::Bytes;
use futures::Stream;
use http_body_util::BodyExt as _;
use open_compute_artifacts::{
    ArtifactRef, ArtifactStore, MapEnv, MockS3, S3ArtifactClient, resolve_s3_credentials_with,
};
use open_compute_core::clock::SystemClock;
use open_compute_core::config::{PlatformConfig, RuntimeConfig, StorageConfig};
use open_compute_core::{
    ComponentName, ComponentState, ErrorCode, MetricsConfig, QueueMessageId, ReadinessReason,
    Redactor, RequestId, SecretString, ServerConfig,
};
use open_compute_runtime::{
    DirectoryServicePath, ExternalServiceAddress, GenerationAuthRegistry, OsJitter,
    PlatformReleaseMeta, StaticConfigCompiler, SupervisorState, WorkerdSupervisor,
    WorkerdSupervisorOptions, verify_runtime_binary,
};
use open_compute_service::http::{HttpState, merged_router};
use open_compute_service::runtime_bridge::{
    DispatchTarget, LoaderOutcome, QueueDispatchMessage, QueueDispatchRequest,
    ScheduledDispatchRequest, WorkerdTransport, bind_runtime_source, serve_runtime_source,
};
use open_compute_service::workers_http::WorkerApiState;
use open_compute_service::{
    HealthCoordinator, MetricsRegistry, SqliteKvBindingExecutor, bind_binding_backend,
    serve_binding_backend,
};
use open_compute_storage::{
    DeploymentState, PlatformStorage, QueueContentType, SchedulerStore, WorkerRepository,
};
use open_compute_workers::{
    BundleLimits, CanonicalBundle, CreateDeploymentOutcome, CreateDeploymentRequest,
    DeploymentController, DeploymentPins, ModuleInput, ModuleType, ResourcePins, RuntimeSource,
    RuntimeValidator,
};
use std::collections::BTreeMap;
use std::convert::Infallible;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};
use tower::ServiceExt;

#[path = "p0_2_runtime_gate/http.rs"]
mod http;
#[path = "p0_2_runtime_gate/nodejs.rs"]
mod nodejs;
#[path = "p0_2_runtime_gate/worker_toolchain.rs"]
mod worker_toolchain;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn p0_2_real_worker_create_validate_dispatch_promote_rollback_restart() {
    let workerd = std::env::var_os("OPEN_COMPUTE_TEST_WORKERD")
        .map(PathBuf::from)
        .expect("OPEN_COMPUTE_TEST_WORKERD must name the verified stock runtime");
    let root = repo_root();
    let lock = root.join("packages/runtime/workerd.lock.json");
    let assets = root.join("packages/runtime");
    let temp = tempfile::tempdir().unwrap();
    let storage = Arc::new(
        PlatformStorage::bootstrap(&storage_config(&temp.path().join("data")), &SystemClock)
            .unwrap(),
    );
    let scheduler = Arc::new(
        SchedulerStore::open(&storage.data_dir().ensure_scheduler_db().unwrap(), 100, 1).unwrap(),
    );
    let mock = MockS3::spawn("open-compute").await;
    let artifacts = artifact_store(&mock);
    let runtime = verify_runtime_binary(&lock, &workerd, Duration::from_secs(10), &Redactor::new())
        .await
        .expect("formal pinned runtime");

    let auth = GenerationAuthRegistry::new();
    let binding_auth = GenerationAuthRegistry::new();
    let source_listener = bind_runtime_source().await.unwrap();
    let source_addr = source_listener.local_addr().unwrap();
    let binding_listener = bind_binding_backend().await.unwrap();
    let binding_addr = binding_listener.local_addr().unwrap();
    let (shutdown_tx, mut shutdown_rx) = tokio::sync::watch::channel(false);
    let mut binding_shutdown_rx = shutdown_tx.subscribe();
    let source_task = tokio::spawn({
        let source =
            RuntimeSource::new(storage.clone(), artifacts.clone(), BundleLimits::default());
        let auth = auth.clone();
        async move {
            serve_runtime_source(source_listener, source, auth, async move {
                let _ = shutdown_rx.changed().await;
            })
            .await
        }
    });
    let binding_task = tokio::spawn({
        let storage = storage.clone();
        let auth = binding_auth.clone();
        async move {
            serve_binding_backend(
                binding_listener,
                storage.clone(),
                auth,
                ResourcePins::new(),
                Arc::new(SqliteKvBindingExecutor::new(storage, Arc::new(SystemClock))),
                None,
                None,
                None,
                open_compute_core::DurableObjectsConfig::default(),
                open_compute_core::QueuesConfig::default(),
                open_compute_core::WorkflowsConfig::default(),
                None,
                async move {
                    let _ = binding_shutdown_rx.changed().await;
                },
            )
            .await
        }
    });

    let compiler = StaticConfigCompiler::new(
        runtime.clone(),
        lock.clone(),
        assets,
        storage.data_dir().runtime_dir(),
        PlatformReleaseMeta {
            version: "p0.2-gate".to_owned(),
        },
        Duration::from_secs(20),
        Redactor::new(),
    )
    .with_generation_auth(auth.clone())
    .with_binding_generation_auth(binding_auth.clone());
    let supervisor_slot = Arc::new(Mutex::new(None));
    let transport = WorkerdTransport::new(auth.clone(), supervisor_slot.clone());
    let do_storage = storage
        .data_dir()
        .prepare_durable_object_storage(
            &storage.identity().platform_id.to_string(),
            runtime.version_output(),
        )
        .unwrap();
    let supervisor = Arc::new(WorkerdSupervisor::new(
        WorkerdSupervisorOptions {
            runtime,
            compiler,
            config: runtime_config(),
            clock: Arc::new(SystemClock),
            jitter: Arc::new(OsJitter),
            redactor: Redactor::new(),
            lease_path: Some(storage.data_dir().runtime_dir().join("p0-2-gate.lease")),
        },
        vec![
            ExternalServiceAddress::loopback("runtime-source", source_addr).unwrap(),
            ExternalServiceAddress::loopback("binding-backend", binding_addr).unwrap(),
        ],
        vec![DirectoryServicePath::local("do-storage", &do_storage).unwrap()],
        vec![auth.clone(), binding_auth.clone()],
    ));
    *supervisor_slot.lock().unwrap() = Some(supervisor.clone());
    supervisor.start();
    wait_running(&supervisor, Duration::from_secs(30)).await;
    let first_pid = supervisor.snapshot().pid.unwrap();
    let first_credential = auth.credential().unwrap();

    let account = storage.identity().default_account_id;
    let repo = WorkerRepository::new(storage.db());
    let (worker, _) = repo
        .create_worker(account, "runtime-gate", RequestId::generate(), 1, 1_000_000)
        .unwrap();
    let validator: Arc<dyn RuntimeValidator> = Arc::new(transport.clone());
    let controller = DeploymentController::new(
        &storage,
        artifacts.clone(),
        validator,
        BundleLimits::default(),
    )
    .with_product_promoter(open_compute_service::product_promotion_for_test(
        storage.clone(),
        scheduler,
    ));

    let a = deploy(
        &controller,
        account,
        worker.id,
        "deploy-a",
        "A",
        true,
        false,
    )
    .await;
    assert_eq!(
        supervisor.snapshot().state,
        SupervisorState::Running,
        "runtime left Running after deployment validation: {:?}",
        supervisor.last_diagnostics()
    );
    assert!(
        !source_task.is_finished(),
        "runtime-source server stopped during deployment validation"
    );
    assert!(
        !binding_task.is_finished(),
        "binding-backend server stopped during deployment validation"
    );
    let response = dispatch(&transport, account, worker.id, &a, None, "hello-a").await;
    assert_eq!(
        response.status, 200,
        "unexpected dispatch response: {response:?}"
    );
    assert_eq!(response.loader_outcome, Some(LoaderOutcome::Cold));
    assert!(response.body.contains("A:hello-a:production:gate-secret"));
    assert!(response.body.ends_with(":API_TOKEN,MODE"));

    // Warm path is still descriptor-resolved and produces the same immutable code.
    let warm = dispatch(&transport, account, worker.id, &a, None, "warm").await;
    assert_eq!(warm.status, 200);
    assert_eq!(warm.loader_outcome, Some(LoaderOutcome::Warm));
    assert!(warm.body.contains("A:warm:production:gate-secret"));

    // Native Queue and scheduled custom events traverse the same immutable dynamic loader.
    let queue_ids = [
        QueueMessageId::generate(),
        QueueMessageId::generate(),
        QueueMessageId::generate(),
    ];
    let queue_result = transport
        .dispatch_queue(
            &dispatch_target(account, worker.id, &a, None),
            &QueueDispatchRequest {
                queue_name: "runtime-gate".to_owned(),
                messages: vec![
                    QueueDispatchMessage {
                        id: queue_ids[0].to_string(),
                        timestamp_ms: 1_787_700_000_000,
                        attempts: 1,
                        content_type: QueueContentType::Text,
                        body_base64: base64::engine::general_purpose::STANDARD.encode("ack"),
                    },
                    QueueDispatchMessage {
                        id: queue_ids[1].to_string(),
                        timestamp_ms: 1_787_700_000_001,
                        attempts: 2,
                        content_type: QueueContentType::Json,
                        body_base64: base64::engine::general_purpose::STANDARD
                            .encode(br#"{"action":"retry"}"#),
                    },
                    QueueDispatchMessage {
                        id: queue_ids[2].to_string(),
                        timestamp_ms: 1_787_700_000_002,
                        attempts: 3,
                        content_type: QueueContentType::Bytes,
                        body_base64: base64::engine::general_purpose::STANDARD.encode([0, 255, 7]),
                    },
                ],
                metadata: Default::default(),
            },
            Duration::from_secs(5),
        )
        .await;
    let queue_result = match queue_result {
        Ok(result) => result,
        Err(error) => {
            supervisor.shutdown().await;
            panic!(
                "native Queue custom event: {error:?}; diagnostics: {:?}",
                supervisor.last_diagnostics()
            );
        }
    };
    assert_eq!(queue_result.outcome, "ok");
    assert_eq!(queue_result.explicit_acks, [queue_ids[0].to_string()]);
    assert_eq!(queue_result.retry_messages.len(), 1);
    assert_eq!(
        queue_result.retry_messages[0].msg_id,
        queue_ids[1].to_string()
    );
    assert_eq!(queue_result.retry_messages[0].delay_seconds, Some(7));
    assert!(!queue_result.ack_all);
    assert!(!queue_result.retry_batch.retry);

    let scheduled = transport
        .dispatch_scheduled(
            &dispatch_target(account, worker.id, &a, None),
            &ScheduledDispatchRequest {
                scheduled_time_ms: 1_787_700_060_000,
                cron: "*/5 * * * *".to_owned(),
                scheduled_handler: true,
                workflow_bindings: Vec::new(),
            },
            Duration::from_secs(5),
        )
        .await
        .expect("native scheduled custom event");
    assert_eq!(scheduled.outcome, "ok");
    assert!(scheduled.no_retry);

    for (queue_name, expected) in [
        ("runtime-gate-throw", "exception"),
        ("runtime-gate-wait-until", "exception"),
    ] {
        let result = transport
            .dispatch_queue(
                &dispatch_target(account, worker.id, &a, None),
                &QueueDispatchRequest {
                    queue_name: queue_name.to_owned(),
                    messages: vec![QueueDispatchMessage {
                        id: QueueMessageId::generate().to_string(),
                        timestamp_ms: 1_787_700_000_000,
                        attempts: 1,
                        content_type: QueueContentType::Text,
                        body_base64: base64::engine::general_purpose::STANDARD.encode("failure"),
                    }],
                    metadata: Default::default(),
                },
                Duration::from_secs(5),
            )
            .await
            .expect("known Queue failure result");
        assert_eq!(result.outcome, expected);
    }
    for cron in ["1 * * * *", "2 * * * *"] {
        let result = transport
            .dispatch_scheduled(
                &dispatch_target(account, worker.id, &a, None),
                &ScheduledDispatchRequest {
                    scheduled_time_ms: 1_787_700_060_000,
                    cron: cron.to_owned(),
                    scheduled_handler: true,
                    workflow_bindings: Vec::new(),
                },
                Duration::from_secs(5),
            )
            .await
            .expect("known scheduled failure result");
        assert_eq!(result.outcome, "exception");
        assert!(!result.no_retry);
    }

    let named_queue = transport
        .dispatch_queue(
            &dispatch_target(account, worker.id, &a, Some("Named")),
            &QueueDispatchRequest {
                queue_name: "runtime-gate".to_owned(),
                messages: vec![QueueDispatchMessage {
                    id: QueueMessageId::generate().to_string(),
                    timestamp_ms: 1_787_700_000_003,
                    attempts: 1,
                    content_type: QueueContentType::Text,
                    body_base64: base64::engine::general_purpose::STANDARD.encode("named"),
                }],
                metadata: Default::default(),
            },
            Duration::from_secs(5),
        )
        .await
        .expect("named Queue custom event");
    assert_eq!(named_queue.outcome, "ok");
    assert!(named_queue.ack_all);

    let named = dispatch(&transport, account, worker.id, &a, Some("Named"), "named").await;
    assert_eq!(named.status, 200, "unexpected named response: {named:?}");
    assert_eq!(named.body, "named:A:named");
    let missing = dispatch(
        &transport,
        account,
        worker.id,
        &a,
        Some("Missing"),
        "missing",
    )
    .await;
    assert_eq!(missing.status, 404);
    assert!(missing.body.contains("ENTRYPOINT_NOT_FOUND"));

    let conformance = dispatch(&transport, account, worker.id, &a, None, "conformance").await;
    assert_eq!(conformance.status, 200);
    let conformance: serde_json::Value = serde_json::from_str(&conformance.body).unwrap();
    for api in [
        "fetch",
        "request",
        "response",
        "headers",
        "url",
        "streams",
        "crypto",
        "timers",
        "webSocket",
    ] {
        assert_eq!(conformance[api], true, "conformance API {api}");
    }

    // The platform proxy keeps both directions streaming. The echo path does
    // not materialize the request in platformd, and an early tenant response
    // cancels a request producer that has not reached EOF.
    let stream_payload = vec![b's'; 4 * 1024 * 1024];
    let stream = futures::stream::iter(
        stream_payload
            .chunks(32 * 1024)
            .map(|chunk| Ok::<Bytes, Infallible>(Bytes::copy_from_slice(chunk)))
            .collect::<Vec<_>>(),
    );
    let stream_response = transport
        .dispatch(
            dispatch_target(account, worker.id, &a, None),
            Request::builder()
                .method("POST")
                .uri("/runtime-gate/stream")
                .header(header::HOST, "workers.example.test")
                .body(Body::from_stream(stream))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(stream_response.status(), 200);
    assert_eq!(
        to_bytes(stream_response.into_body(), 5 * 1024 * 1024)
            .await
            .unwrap()
            .as_ref(),
        stream_payload
    );

    let producer_dropped = Arc::new(AtomicBool::new(false));
    let early_response = tokio::time::timeout(
        Duration::from_secs(3),
        transport.dispatch(
            dispatch_target(account, worker.id, &a, None),
            Request::builder()
                .method("POST")
                .uri("/runtime-gate/early")
                .header(header::HOST, "workers.example.test")
                .body(Body::from_stream(PendingUpload {
                    first: Some(Bytes::from_static(b"first-chunk")),
                    dropped: producer_dropped.clone(),
                }))
                .unwrap(),
        ),
    )
    .await
    .expect("tenant must be able to respond before request EOF")
    .unwrap();
    assert_eq!(
        to_bytes(early_response.into_body(), 1024).await.unwrap(),
        "early-response"
    );
    tokio::time::timeout(Duration::from_secs(3), async {
        while !producer_dropped.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("early response must stop polling and drop the upload stream");

    let b = deploy(
        &controller,
        account,
        worker.id,
        "deploy-b",
        "B",
        true,
        false,
    )
    .await;
    assert_ne!(a.id, b.id);
    assert_eq!(
        repo.get_worker(account, worker.id)
            .unwrap()
            .active_deployment_id,
        Some(b.id)
    );

    let egress_fixture = egress_fixture_from_env();
    if let Some(fixture) = egress_fixture.as_ref() {
        run_tls_fixture(&workerd, &root, fixture).await;
    }
    let egress = deploy_egress(&controller, account, worker.id, egress_fixture.as_ref()).await;
    let denied = dispatch(&transport, account, worker.id, &egress, None, "").await;
    assert_eq!(
        denied.status,
        200,
        "egress response: {denied:?}; diagnostics: {:?}",
        supervisor.last_diagnostics()
    );
    let egress_result: serde_json::Value = serde_json::from_str(&denied.body).unwrap();
    let expected_denied = if egress_fixture.is_some() { 11 } else { 9 };
    assert_eq!(egress_result["denied"], expected_denied);
    let allowed = egress_result["allowed"].as_array().unwrap();
    assert_eq!(allowed.len(), egress_fixture.as_ref().map_or(0, |_| 3));
    assert!(allowed.iter().all(|value| value == "fixture-ok"));
    assert_eq!(
        egress_result["ctxExports"]["ok"], true,
        "ctx.exports connect: {}",
        egress_result["ctxExports"]
    );
    assert_eq!(egress_result["ctxExports"]["bytes"], 96 * 1024);
    assert_eq!(
        egress_result["ctxExports"]["localAddress"],
        "loopback.invalid:7000"
    );
    assert_eq!(
        egress_result["ctxExports"]["remoteAddress"],
        serde_json::Value::Null
    );
    assert!(
        egress_result["ctxExports"]["chunks"]
            .as_u64()
            .is_some_and(|chunks| chunks > 1),
        "ctx.exports socket echo must cross stream chunks: {}",
        egress_result["ctxExports"]
    );
    if let Some(egress_fixture) = &egress_fixture {
        assert_raw_tcp_fixture(&egress_result["rawTcp"], egress_fixture);
        let event_message = QueueMessageId::generate();
        let event_queue = transport
            .dispatch_queue(
                &dispatch_target(account, worker.id, &egress, None),
                &QueueDispatchRequest {
                    queue_name: "raw-tcp-event-source".to_owned(),
                    messages: vec![QueueDispatchMessage {
                        id: event_message.to_string(),
                        timestamp_ms: 1_787_700_000_010,
                        attempts: 1,
                        content_type: QueueContentType::Text,
                        body_base64: base64::engine::general_purpose::STANDARD.encode("socket"),
                    }],
                    metadata: Default::default(),
                },
                Duration::from_secs(10),
            )
            .await
            .expect("Queue raw TCP event source");
        assert_eq!(event_queue.outcome, "ok");
        assert!(event_queue.ack_all);
        let event_scheduled = transport
            .dispatch_scheduled(
                &dispatch_target(account, worker.id, &egress, None),
                &ScheduledDispatchRequest {
                    scheduled_time_ms: 1_787_700_060_000,
                    cron: "3 * * * *".to_owned(),
                    scheduled_handler: true,
                    workflow_bindings: Vec::new(),
                },
                Duration::from_secs(10),
            )
            .await
            .expect("scheduled raw TCP event source");
        assert_eq!(event_scheduled.outcome, "ok");
        assert!(event_scheduled.no_retry);
    } else {
        assert_eq!(egress_result["rawTcp"], serde_json::Value::Null);
    }
    let node = deploy_node(&controller, account, worker.id).await;
    let node_response = dispatch(&transport, account, worker.id, &node, None, "").await;
    assert_eq!(node_response.status, 200);
    assert_eq!(node_response.body, "node-compat");
    assert!(
        dispatch(&transport, account, worker.id, &b, None, "active-b")
            .await
            .body
            .contains("B:active-b")
    );
    repo.promote(
        account,
        worker.id,
        a.id,
        Some(b.id),
        RequestId::generate(),
        10,
    )
    .unwrap();
    assert!(
        dispatch(&transport, account, worker.id, &a, None, "rollback-a")
            .await
            .body
            .contains("A:rollback-a")
    );

    // Deterministic parse/startup failure is rejected and cannot move active.
    let active_before = repo
        .get_worker(account, worker.id)
        .unwrap()
        .active_deployment_id;
    let invalid = create_request(account, worker.id, "deploy-invalid", "C", false, true);
    let error = controller.create_deployment(invalid).await.unwrap_err();
    assert_eq!(error.code(), ErrorCode::BundleRuntimeInvalid);
    assert_eq!(
        repo.get_worker(account, worker.id)
            .unwrap()
            .active_deployment_id,
        active_before
    );
    assert_eq!(
        repo.list_deployments(account, worker.id).unwrap()[0].state,
        DeploymentState::Rejected
    );

    // Restart rotates credentials and forces a new workerd process/cold cache.
    supervisor.report_unhealthy();
    wait_pid_change(&supervisor, first_pid, Duration::from_secs(30)).await;
    assert_ne!(
        auth.credential().unwrap().expose(),
        first_credential.expose(),
        "generation token must rotate"
    );
    let concurrent = futures::future::join_all((0..100).map(|index| {
        let transport = transport.clone();
        let deployment = a.clone();
        async move {
            dispatch(
                &transport,
                account,
                worker.id,
                &deployment,
                None,
                &format!("restart-{index}"),
            )
            .await
        }
    }))
    .await;
    assert!(concurrent.iter().all(|response| response.status == 200));
    assert_eq!(
        concurrent
            .iter()
            .filter(|response| response.loader_outcome == Some(LoaderOutcome::Cold))
            .count(),
        1,
        "100 concurrent cold requests must invoke exactly one native loader callback"
    );

    // The stable HTTP surface drives the same real validation and dispatch path.
    http::api_matrix(
        storage.clone(),
        artifacts.clone(),
        transport.clone(),
        account,
    )
    .await;

    // Once response headers/body have started, a runtime crash truncates the
    // stream. platformd must not rewrite or replay it as a clean JSON error.
    let crash_pid = supervisor.snapshot().pid.unwrap();
    let timeout = transport
        .dispatch_queue(
            &dispatch_target(account, worker.id, &a, None),
            &QueueDispatchRequest {
                queue_name: "runtime-gate-timeout".to_owned(),
                messages: vec![QueueDispatchMessage {
                    id: QueueMessageId::generate().to_string(),
                    timestamp_ms: 1_787_700_000_000,
                    attempts: 1,
                    content_type: QueueContentType::Text,
                    body_base64: base64::engine::general_purpose::STANDARD.encode("timeout"),
                }],
                metadata: Default::default(),
            },
            Duration::from_millis(100),
        )
        .await
        .unwrap_err();
    assert_eq!(timeout.code(), ErrorCode::QueueSendResultUnknown);
    let crash_response = transport
        .dispatch(
            dispatch_target(account, worker.id, &a, None),
            Request::builder()
                .method("GET")
                .uri("/runtime-gate/midstream")
                .header(header::HOST, "workers.example.test")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(crash_response.status(), 200);
    let mut crash_body = crash_response.into_body();
    let first = tokio::time::timeout(Duration::from_secs(3), crash_body.frame())
        .await
        .expect("midstream prefix deadline")
        .expect("midstream prefix frame")
        .expect("midstream prefix transport");
    assert_eq!(first.into_data().expect("data frame"), "stream-prefix");
    supervisor.report_unhealthy();
    wait_pid_change(&supervisor, crash_pid, Duration::from_secs(30)).await;
    let tail = tokio::time::timeout(Duration::from_secs(3), crash_body.frame())
        .await
        .expect("crashed response stream must terminate");
    assert!(
        tail.is_none() || tail.is_some_and(|frame| frame.is_err()),
        "a started response must truncate, not become a platform error body"
    );
    assert_eq!(
        dispatch(&transport, account, worker.id, &a, None, "post-crash")
            .await
            .status,
        200
    );
    let restarted_queue = transport
        .dispatch_queue(
            &dispatch_target(account, worker.id, &a, None),
            &QueueDispatchRequest {
                queue_name: "runtime-gate-throw".to_owned(),
                messages: vec![QueueDispatchMessage {
                    id: QueueMessageId::generate().to_string(),
                    timestamp_ms: 1_787_700_000_000,
                    attempts: 1,
                    content_type: QueueContentType::Text,
                    body_base64: base64::engine::general_purpose::STANDARD.encode("restart"),
                }],
                metadata: Default::default(),
            },
            Duration::from_secs(5),
        )
        .await
        .expect("Queue custom event after restart");
    assert_eq!(restarted_queue.outcome, "exception");
    let restarted_scheduled = transport
        .dispatch_scheduled(
            &dispatch_target(account, worker.id, &a, None),
            &ScheduledDispatchRequest {
                scheduled_time_ms: 1_787_700_060_000,
                cron: "1 * * * *".to_owned(),
                scheduled_handler: true,
                workflow_bindings: Vec::new(),
            },
            Duration::from_secs(5),
        )
        .await
        .expect("scheduled custom event after restart");
    assert_eq!(restarted_scheduled.outcome, "exception");

    // A warm WorkerLoader entry must not bypass the pre-get source/descriptor
    // check. Corrupting the authority after warm load fails closed instead of
    // executing the already-cached isolate.
    let artifact = ArtifactRef::new(
        1,
        &hex::encode(a.artifact_sha256.unwrap()),
        a.artifact_size.unwrap(),
    )
    .unwrap();
    mock.corrupt_body(&artifact.physical_key("system/"));
    let warm_corrupt = dispatch(&transport, account, worker.id, &a, None, "must-not-run").await;
    assert_eq!(warm_corrupt.status, 500);
    assert!(warm_corrupt.body.contains("ARTIFACT_INTEGRITY_ERROR"));

    supervisor.shutdown().await;
    let _ = shutdown_tx.send(true);
    source_task.await.unwrap().unwrap();
    binding_task.await.unwrap().unwrap();
    assert!(supervisor.snapshot().pid.is_none());
}

async fn deploy_egress(
    controller: &DeploymentController<'_>,
    account: open_compute_core::AccountId,
    worker: open_compute_core::WorkerId,
    fixture: Option<&EgressFixture>,
) -> open_compute_storage::DeploymentRecord {
    let public_targets = fixture.map_or_else(Vec::new, |fixture| {
        vec![
            fixture.public_ipv4_url.clone(),
            fixture.public_ipv6_url.clone(),
            fixture.public_hostname_url.clone(),
        ]
    });
    let mut denied_targets = vec![
        "http://127.0.0.1:1/".to_owned(),
        "http://10.0.0.1/".to_owned(),
        "http://169.254.169.254/latest/meta-data/".to_owned(),
        "http://[::1]/".to_owned(),
        "http://[::ffff:127.0.0.1]/".to_owned(),
        "http://2130706433/".to_owned(),
        "http://user@127.0.0.1/".to_owned(),
        "http://localhost/".to_owned(),
        "file:///etc/passwd".to_owned(),
    ];
    if let Some(fixture) = fixture {
        denied_targets.push(fixture.redirect_private_url.clone());
        denied_targets.push(fixture.private_hostname_url.clone());
    }
    let mut vars = BTreeMap::new();
    vars.insert(
        "PUBLIC_TARGETS_JSON".to_owned(),
        serde_json::json!(serde_json::to_string(&public_targets).unwrap()),
    );
    vars.insert(
        "DENIED_TARGETS_JSON".to_owned(),
        serde_json::json!(serde_json::to_string(&denied_targets).unwrap()),
    );
    if let Some(fixture) = fixture {
        vars.insert(
            "RAW_TCP_CONFIG_JSON".to_owned(),
            serde_json::json!(
                serde_json::json!({
                    "ipv4Host": fixture.public_ipv4_host,
                    "ipv6Host": fixture.public_ipv6_host,
                    "hostname": fixture.public_hostname,
                    "privateHostname": fixture.private_hostname,
                    "tcpPort": fixture.public_tcp_port,
                    "tlsPort": fixture.public_tls_port,
                })
                .to_string()
            ),
        );
    }
    let bundle = CanonicalBundle::build(
        "index.js",
        vec![ModuleInput {
            name: "index.js".to_owned(),
            module_type: ModuleType::EsModule,
            bytes: include_bytes!("../../../test/runtime/fixtures/p0-2-egress.js").to_vec(),
        }],
        BundleLimits::default(),
    )
    .unwrap();
    let request = CreateDeploymentRequest {
        account_id: account,
        worker_id: worker,
        idempotency_key: "deploy-egress".to_owned(),
        content: open_compute_workers::DeploymentContent::Worker {
            bundle: bundle.into_bytes().into(),
            assets: None,
        },
        vars,
        secrets: BTreeMap::new(),
        bindings: BTreeMap::new(),
        services: BTreeMap::new(),
        runtime_features: Default::default(),
        queue_consumers: Vec::new(),
        crons: vec!["3 * * * *".to_owned()],
        promote: false,
        request_id: RequestId::generate(),
        now_ms: 20,
    };
    match controller.create_deployment(request).await.unwrap() {
        CreateDeploymentOutcome::Applied(result) => result.deployment,
        CreateDeploymentOutcome::Replay(_) => panic!("unexpected replay"),
    }
}

#[derive(Debug)]
struct EgressFixture {
    public_ipv4_url: String,
    public_ipv6_url: String,
    public_hostname_url: String,
    redirect_private_url: String,
    private_hostname_url: String,
    public_ipv4_host: String,
    public_ipv6_host: String,
    public_hostname: String,
    private_hostname: String,
    public_tcp_port: String,
    public_tls_port: String,
    tls_ca_path: PathBuf,
}

fn egress_fixture_from_env() -> Option<EgressFixture> {
    const NAMES: [&str; 12] = [
        "OPEN_COMPUTE_EGRESS_PUBLIC_IPV4_URL",
        "OPEN_COMPUTE_EGRESS_PUBLIC_IPV6_URL",
        "OPEN_COMPUTE_EGRESS_PUBLIC_HOSTNAME_URL",
        "OPEN_COMPUTE_EGRESS_REDIRECT_PRIVATE_URL",
        "OPEN_COMPUTE_EGRESS_PRIVATE_HOSTNAME_URL",
        "OPEN_COMPUTE_EGRESS_PUBLIC_IPV4_HOST",
        "OPEN_COMPUTE_EGRESS_PUBLIC_IPV6_HOST",
        "OPEN_COMPUTE_EGRESS_PUBLIC_HOSTNAME",
        "OPEN_COMPUTE_EGRESS_PRIVATE_HOSTNAME",
        "OPEN_COMPUTE_EGRESS_PUBLIC_TCP_PORT",
        "OPEN_COMPUTE_EGRESS_PUBLIC_TLS_PORT",
        "OPEN_COMPUTE_EGRESS_TLS_CA_PATH",
    ];
    let values = NAMES.map(std::env::var);
    if values.iter().all(Result::is_err) {
        return None;
    }
    let [
        public_ipv4_url,
        public_ipv6_url,
        public_hostname_url,
        redirect_private_url,
        private_hostname_url,
        public_ipv4_host,
        public_ipv6_host,
        public_hostname,
        private_hostname,
        public_tcp_port,
        public_tls_port,
        tls_ca_path,
    ] = values.map(|value| value.expect("all controlled egress fixture URLs must be set"));
    Some(EgressFixture {
        public_ipv4_url,
        public_ipv6_url,
        public_hostname_url,
        redirect_private_url,
        private_hostname_url,
        public_ipv4_host,
        public_ipv6_host,
        public_hostname,
        private_hostname,
        public_tcp_port,
        public_tls_port,
        tls_ca_path: PathBuf::from(tls_ca_path),
    })
}

async fn run_tls_fixture(workerd: &Path, root: &Path, fixture: &EgressFixture) {
    let temp = tempfile::tempdir().expect("TLS fixture tempdir");
    for name in ["p0-2-tls.wd-test", "p0-2-tls.js"] {
        std::fs::copy(
            root.join("test/runtime/fixtures").join(name),
            temp.path().join(name),
        )
        .expect("copy TLS fixture source");
    }
    std::fs::copy(&fixture.tls_ca_path, temp.path().join("ca.pem")).expect("copy TLS fixture CA");
    for test in ["cloudflareTlsOn", "cloudflareStartTls", "nodeTlsLifecycle"] {
        let mut command = tokio::process::Command::new(workerd);
        command
            .arg("test")
            .arg(temp.path().join("p0-2-tls.wd-test"))
            .arg("--experimental")
            .arg(format!("p0-2-tls:{test}"))
            .env(
                "OPEN_COMPUTE_EGRESS_PUBLIC_TLS_PORT",
                &fixture.public_tls_port,
            )
            .env(
                "OPEN_COMPUTE_EGRESS_PUBLIC_IPV4_HOST",
                &fixture.public_ipv4_host,
            )
            .kill_on_drop(true);
        let output = tokio::time::timeout(Duration::from_secs(15), command.output())
            .await
            .unwrap_or_else(|_| panic!("TLS fixture timed out: {test}"))
            .unwrap_or_else(|error| panic!("TLS fixture failed to start ({test}): {error}"));
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success(),
            "TLS fixture failed ({test}): {stderr}"
        );
        assert!(
            stderr.contains(&format!("[ PASS ] p0-2-tls:{test}")) && !stderr.contains("[ FAIL ]"),
            "TLS fixture did not report a clean pass ({test}): {stderr}"
        );
    }
}

fn assert_raw_tcp_fixture(raw: &serde_json::Value, fixture: &EgressFixture) {
    let sockets = &raw["sockets"];
    let expected_authorities = [
        (
            "ipv4",
            format!("{}:{}", fixture.public_ipv4_host, fixture.public_tcp_port),
        ),
        (
            "ipv6",
            format!("[{}]:{}", fixture.public_ipv6_host, fixture.public_tcp_port),
        ),
        (
            "dns",
            format!("{}:{}", fixture.public_hostname, fixture.public_tcp_port),
        ),
    ];
    for (name, expected_authority) in expected_authorities {
        assert_eq!(
            sockets[name]["bytes"],
            192 * 1024,
            "{name} socket echo: {}",
            sockets[name]
        );
        assert!(
            sockets[name]["chunks"]
                .as_u64()
                .is_some_and(|chunks| chunks > 1),
            "{name} socket echo must cross stream chunks: {}",
            sockets[name]
        );
        assert_eq!(
            sockets[name]["localAddress"],
            serde_json::Value::Null,
            "{name} outbound socket must not invent a local address"
        );
        assert_eq!(
            sockets[name]["remoteAddress"], expected_authority,
            "{name} outbound socket must preserve the requested authority"
        );
    }
    assert_eq!(
        sockets["ipv4"]["initialDesiredSize"], 4096,
        "highWaterMark must configure the writable stream"
    );
    assert_eq!(
        sockets["halfOpenFalse"]["marker"], "peer-half-close",
        "{}",
        sockets["halfOpenFalse"]
    );
    assert_eq!(sockets["halfOpenFalse"]["writeAfterEof"], false);
    assert_eq!(
        sockets["halfOpenTrue"]["marker"], "peer-half-close",
        "{}",
        sockets["halfOpenTrue"]
    );
    assert_eq!(sockets["halfOpenTrue"]["writeAfterEof"], true);
    assert_eq!(sockets["halfOpenTrue"]["closeError"], false);
    assert_eq!(
        sockets["tlsOn"]["certificateRejected"], true,
        "{}",
        sockets["tlsOn"]
    );
    assert_eq!(sockets["tlsOn"]["initialSecureTransport"], "on");
    assert_eq!(sockets["tlsOn"]["initialUpgraded"], false);
    assert_eq!(
        sockets["startTls"]["certificateRejected"], true,
        "{}",
        sockets["startTls"]
    );
    assert_eq!(sockets["startTls"]["initialSecureTransport"], "starttls");
    assert_eq!(sockets["startTls"]["initialUpgraded"], false);
    assert_eq!(sockets["startTls"]["oldSocketNeutered"], true);
    for name in ["privateDns", "loopback"] {
        assert_eq!(sockets[name]["opened"], false, "{name} raw socket");
        assert_eq!(sockets[name]["denied"], true, "{name} raw socket");
    }

    let node = &raw["node"];
    assert_eq!(
        node["net"]["bytes"],
        192 * 1024,
        "node net echo: {}",
        node["net"]
    );
    assert!(
        node["net"]["chunks"]
            .as_u64()
            .is_some_and(|chunks| chunks > 1),
        "node net echo must cross stream chunks: {}",
        node["net"]
    );
    assert_eq!(node["net"]["destroyed"], true);
    assert_eq!(node["tls"]["certificateRejected"], true, "{}", node["tls"]);
    assert_eq!(node["tls"]["errorEvent"], true, "{}", node["tls"]);
    assert_eq!(node["tls"]["destroyed"], true, "{}", node["tls"]);
    assert_eq!(node["timeout"]["timedOut"], true);
    assert_eq!(node["timeout"]["destroyed"], true);
    for name in ["privateDns", "loopback"] {
        assert_eq!(node[name]["opened"], false, "node {name}");
        assert_eq!(node[name]["denied"], true, "node {name}");
    }
}

async fn deploy_node(
    controller: &DeploymentController<'_>,
    account: open_compute_core::AccountId,
    worker: open_compute_core::WorkerId,
) -> open_compute_storage::DeploymentRecord {
    let bundle = CanonicalBundle::build(
        "index.js",
        vec![ModuleInput {
            name: "index.js".to_owned(),
            module_type: ModuleType::EsModule,
            bytes: br#"import { Buffer } from "node:buffer";
export default { fetch() { return new Response(Buffer.from("node-compat").toString()); } };"#
                .to_vec(),
        }],
        BundleLimits::default(),
    )
    .unwrap();
    let request = CreateDeploymentRequest {
        account_id: account,
        worker_id: worker,
        idempotency_key: "deploy-node-compat".to_owned(),
        content: open_compute_workers::DeploymentContent::Worker {
            bundle: bundle.into_bytes().into(),
            assets: None,
        },
        vars: BTreeMap::new(),
        secrets: BTreeMap::new(),
        bindings: BTreeMap::new(),
        services: BTreeMap::new(),
        runtime_features: Default::default(),
        queue_consumers: Vec::new(),
        crons: Vec::new(),
        promote: false,
        request_id: RequestId::generate(),
        now_ms: 21,
    };
    match controller.create_deployment(request).await.unwrap() {
        CreateDeploymentOutcome::Applied(result) => result.deployment,
        CreateDeploymentOutcome::Replay(_) => panic!("unexpected replay"),
    }
}

async fn deploy(
    controller: &DeploymentController<'_>,
    account: open_compute_core::AccountId,
    worker: open_compute_core::WorkerId,
    key: &str,
    label: &str,
    promote: bool,
    invalid: bool,
) -> open_compute_storage::DeploymentRecord {
    match controller
        .create_deployment(create_request(
            account, worker, key, label, promote, invalid,
        ))
        .await
        .unwrap()
    {
        CreateDeploymentOutcome::Applied(result) => result.deployment,
        CreateDeploymentOutcome::Replay(_) => panic!("unexpected replay"),
    }
}

fn create_request(
    account: open_compute_core::AccountId,
    worker: open_compute_core::WorkerId,
    key: &str,
    label: &str,
    promote: bool,
    invalid: bool,
) -> CreateDeploymentRequest {
    let source = if invalid {
        "export default { fetch( {".to_owned()
    } else {
        format!(
            r#"import {{ WorkerEntrypoint }} from "cloudflare:workers";
export class Named extends WorkerEntrypoint {{
  async fetch(request) {{ return new Response("named:{label}:" + await request.text()); }}
  async queue(batch) {{
    if (batch.queue !== "runtime-gate" || batch.messages[0].body !== "named") throw new Error("named queue shape");
    batch.ackAll();
  }}
}}
export default {{
  async fetch(request, env) {{
    const path = new URL(request.url).pathname;
    if (path === "/runtime-gate/stream") return new Response(request.body);
    if (path === "/runtime-gate/early") return new Response("early-response");
    if (path === "/runtime-gate/midstream") return new Response(new ReadableStream({{
      start(controller) {{ controller.enqueue(new TextEncoder().encode("stream-prefix")); }},
      pull() {{ return new Promise(() => {{}}); }}
    }}));
    const content = await request.text();
    if (path === "/runtime-gate/path" && content === "conformance") {{
      const digest = await crypto.subtle.digest("SHA-256", new TextEncoder().encode("gate"));
      await new Promise((resolve) => setTimeout(resolve, 1));
      const stream = new ReadableStream({{ start(controller) {{ controller.close(); }} }});
      return Response.json({{
        fetch: typeof fetch === "function",
        request: typeof Request === "function",
        response: typeof Response === "function",
        headers: typeof Headers === "function",
        url: new URL("https://example.test/a").pathname === "/a",
        streams: stream instanceof ReadableStream,
        crypto: digest.byteLength === 32,
        timers: typeof setTimeout === "function",
        webSocket: typeof WebSocket === "function"
      }});
    }}
    return new Response("{label}:" + content + ":" + env.MODE + ":" + env.API_TOKEN + ":" + Object.keys(env).sort().join(","));
  }},
  async queue(batch, env, ctx) {{
    if (batch.queue === "runtime-gate-throw") throw new Error("queue failure");
    if (batch.queue === "runtime-gate-wait-until") {{
      ctx.waitUntil(Promise.reject(new Error("queue waitUntil failure")));
      return;
    }}
    if (batch.queue === "runtime-gate-timeout") await new Promise((resolve) => setTimeout(resolve, 10000));
    if (batch.queue !== "runtime-gate" || env.MODE !== "production" || batch.messages.length !== 3) throw new Error("queue shape");
    const [text, json, binary] = batch.messages;
    if (text.body !== "ack" || !(text.timestamp instanceof Date) || text.attempts !== 1) throw new Error("text shape");
    if (json.body.action !== "retry" || json.attempts !== 2) throw new Error("json shape");
    if (!(binary.body instanceof Uint8Array) || binary.body[1] !== 255 || binary.attempts !== 3) throw new Error("bytes shape");
    text.ack();
    text.retry({{ delaySeconds: 99 }});
    json.retry({{ delaySeconds: 7 }});
  }},
  async scheduled(controller, env, ctx) {{
    if (controller.cron === "1 * * * *") throw new Error("scheduled failure");
    if (controller.cron === "2 * * * *") {{
      ctx.waitUntil(Promise.reject(new Error("scheduled waitUntil failure")));
      return;
    }}
    if (controller.type !== "scheduled" || controller.cron !== "*/5 * * * *"
        || controller.scheduledTime !== 1787700060000 || env.MODE !== "production") throw new Error("scheduled shape");
    controller.noRetry();
  }}
}};"#
        )
    };
    let bundle = CanonicalBundle::build(
        "index.js",
        vec![ModuleInput {
            name: "index.js".to_owned(),
            module_type: ModuleType::EsModule,
            bytes: source.into_bytes(),
        }],
        BundleLimits::default(),
    )
    .unwrap();
    let mut vars = BTreeMap::new();
    vars.insert("MODE".to_owned(), serde_json::json!("production"));
    let mut secrets = BTreeMap::new();
    secrets.insert("API_TOKEN".to_owned(), SecretString::new("gate-secret"));
    CreateDeploymentRequest {
        account_id: account,
        worker_id: worker,
        idempotency_key: key.to_owned(),
        content: open_compute_workers::DeploymentContent::Worker {
            bundle: bundle.into_bytes().into(),
            assets: None,
        },
        vars,
        secrets,
        bindings: BTreeMap::new(),
        services: BTreeMap::new(),
        runtime_features: Default::default(),
        queue_consumers: Vec::new(),
        crons: vec![
            "*/5 * * * *".to_owned(),
            "1 * * * *".to_owned(),
            "2 * * * *".to_owned(),
        ],
        promote,
        request_id: RequestId::generate(),
        now_ms: 2,
    }
}

#[derive(Debug)]
struct DispatchResponse {
    status: u16,
    body: String,
    loader_outcome: Option<LoaderOutcome>,
}

struct PendingUpload {
    first: Option<Bytes>,
    dropped: Arc<AtomicBool>,
}

impl Stream for PendingUpload {
    type Item = Result<Bytes, Infallible>;

    fn poll_next(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match self.first.take() {
            Some(bytes) => Poll::Ready(Some(Ok(bytes))),
            None => Poll::Pending,
        }
    }
}

impl Drop for PendingUpload {
    fn drop(&mut self) {
        self.dropped.store(true, Ordering::SeqCst);
    }
}

fn dispatch_target(
    account: open_compute_core::AccountId,
    worker: open_compute_core::WorkerId,
    deployment: &open_compute_storage::DeploymentRecord,
    entrypoint: Option<&str>,
) -> DispatchTarget {
    DispatchTarget {
        account_id: account,
        worker_id: worker,
        deployment_id: deployment.id,
        worker_code_sha256: hex::encode(deployment.worker_code_sha256),
        entrypoint: entrypoint.map(str::to_owned),
        route_generation: 1,
        request_id: RequestId::generate(),
    }
}

async fn dispatch(
    transport: &WorkerdTransport,
    account: open_compute_core::AccountId,
    worker: open_compute_core::WorkerId,
    deployment: &open_compute_storage::DeploymentRecord,
    entrypoint: Option<&str>,
    body: &str,
) -> DispatchResponse {
    let request = Request::builder()
        .method("POST")
        .uri("/runtime-gate/path?x=1")
        .header(header::HOST, "workers.example.test")
        .header("x-open-compute-account-id", "forged")
        .body(Body::from(body.to_owned()))
        .unwrap();
    let response = transport
        .dispatch(
            dispatch_target(account, worker, deployment, entrypoint),
            request,
        )
        .await
        .unwrap();
    let status = response.status().as_u16();
    let loader_outcome = response.extensions().get::<LoaderOutcome>().copied();
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    DispatchResponse {
        status,
        body: String::from_utf8(bytes.to_vec()).unwrap(),
        loader_outcome,
    }
}

async fn wait_running(supervisor: &WorkerdSupervisor, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    let mut rx = supervisor.subscribe();
    loop {
        let snapshot = rx.borrow().clone();
        if snapshot.state == SupervisorState::Running {
            return;
        }
        assert!(
            snapshot.state != SupervisorState::Failed,
            "supervisor failed: {snapshot:?}; diagnostics={:?}",
            supervisor.last_diagnostics()
        );
        assert!(Instant::now() < deadline, "supervisor did not become ready");
        tokio::time::timeout(Duration::from_millis(250), rx.changed())
            .await
            .ok();
    }
}

async fn wait_pid_change(supervisor: &WorkerdSupervisor, previous: i32, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    let mut rx = supervisor.subscribe();
    loop {
        let snapshot = rx.borrow().clone();
        if snapshot.state == SupervisorState::Running && snapshot.pid != Some(previous) {
            return;
        }
        assert!(Instant::now() < deadline, "supervisor did not restart");
        tokio::time::timeout(Duration::from_millis(250), rx.changed())
            .await
            .ok();
    }
}

fn runtime_config() -> RuntimeConfig {
    RuntimeConfig {
        startup_timeout_ms: 20_000,
        shutdown_grace_ms: 500,
        drain_timeout_ms: 500,
        kill_timeout_ms: 500,
        restart_budget: 3,
        restart_window_ms: 60_000,
        restart_backoff_initial_ms: 10,
        restart_backoff_max_ms: 100,
    }
}

fn storage_config(root: &Path) -> StorageConfig {
    StorageConfig {
        data_dir: root.to_owned(),
        master_key_file: root.join("keys/master.key"),
        master_key_env: None,
        sqlite_busy_timeout_ms: 5_000,
        free_space_soft_bytes: 1_073_741_824,
        free_space_hard_bytes: 268_435_456,
    }
}

fn artifact_store(mock: &MockS3) -> ArtifactStore {
    let config = PlatformConfig::from_toml_str(&format!(
        r#"
[s3]
endpoint = "{}"
region = "us-east-1"
bucket = "open-compute"
force_path_style = true
access_key_id_env = "S3_ACCESS_KEY_ID"
secret_access_key_env = "S3_SECRET_ACCESS_KEY"
prefix = "system/"
max_retries = 1
retry_backoff_ms = 10
connect_timeout_ms = 500
request_timeout_ms = 3000
"#,
        mock.endpoint
    ))
    .unwrap()
    .s3;
    let env = MapEnv::new()
        .with("S3_ACCESS_KEY_ID", "AKIAEXAMPLEKEYID01")
        .with(
            "S3_SECRET_ACCESS_KEY",
            "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY",
        );
    let credentials = resolve_s3_credentials_with(&config, &env).unwrap();
    ArtifactStore::new(S3ArtifactClient::connect(&config, &credentials, 32 * 1024 * 1024).unwrap())
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_owned()
}
