//! Real pinned-workerd P0.2 dynamic Worker data-plane gate.

use axum::body::{Body, to_bytes};
use axum::http::{Request, header};
use bytes::Bytes;
use futures::Stream;
use http_body_util::BodyExt as _;
use open_compute_artifacts::{
    ArtifactRef, ArtifactStore, MapEnv, MockS3, S3ArtifactClient, resolve_s3_credentials_with,
};
use open_compute_core::clock::SystemClock;
use open_compute_core::config::{PlatformConfig, RuntimeConfig, StorageConfig};
use open_compute_core::{
    ComponentName, ComponentState, ErrorCode, MetricsConfig, ReadinessReason, Redactor, RequestId,
    SecretString, ServerConfig,
};
use open_compute_runtime::{
    ExternalServiceAddress, GenerationAuthRegistry, OsJitter, PlatformReleaseMeta,
    StaticConfigCompiler, SupervisorState, WorkerdSupervisor, WorkerdSupervisorOptions,
    verify_runtime_binary,
};
use open_compute_service::http::{HttpState, merged_router};
use open_compute_service::runtime_bridge::{
    DispatchTarget, LoaderOutcome, WorkerdTransport, bind_runtime_source, serve_runtime_source,
};
use open_compute_service::workers_http::WorkerApiState;
use open_compute_service::{HealthCoordinator, MetricsRegistry};
use open_compute_storage::{DeploymentState, PlatformStorage, WorkerRepository};
use open_compute_workers::{
    BundleLimits, CanonicalBundle, CreateDeploymentOutcome, CreateDeploymentRequest,
    DeploymentController, DeploymentPins, ModuleInput, ModuleType, RuntimeSource, RuntimeValidator,
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn p0_2_real_worker_create_validate_dispatch_promote_rollback_restart() {
    let Some(workerd) = std::env::var_os("OPEN_COMPUTE_TEST_WORKERD").map(PathBuf::from) else {
        // scripts/test-p0-2 makes this mandatory; ordinary cargo test stays offline.
        return;
    };
    let root = repo_root();
    let lock = root.join("runtime/workerd.lock.json");
    let assets = root.join("runtime");
    let temp = tempfile::tempdir().unwrap();
    let storage = Arc::new(
        PlatformStorage::bootstrap(&storage_config(&temp.path().join("data")), &SystemClock)
            .unwrap(),
    );
    let mock = MockS3::spawn("open-compute").await;
    let artifacts = artifact_store(&mock);
    let runtime = verify_runtime_binary(&lock, &workerd, Duration::from_secs(10), &Redactor::new())
        .await
        .expect("formal pinned runtime");

    let auth = GenerationAuthRegistry::new();
    let source_listener = bind_runtime_source().await.unwrap();
    let source_addr = source_listener.local_addr().unwrap();
    let (shutdown_tx, mut shutdown_rx) = tokio::sync::watch::channel(false);
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
    .with_generation_auth(auth.clone());
    let supervisor_slot = Arc::new(Mutex::new(None));
    let transport = WorkerdTransport::new(auth.clone(), supervisor_slot.clone());
    let supervisor = Arc::new(WorkerdSupervisor::new_with_external_services(
        WorkerdSupervisorOptions {
            runtime,
            compiler,
            config: runtime_config(workerd, lock, root.join("runtime")),
            clock: Arc::new(SystemClock),
            jitter: Arc::new(OsJitter),
            redactor: Redactor::new(),
            lease_path: Some(storage.data_dir().runtime_dir().join("p0-2-gate.lease")),
        },
        vec![ExternalServiceAddress::loopback("runtime-source", source_addr).unwrap()],
        Some(auth.clone()),
    ));
    *supervisor_slot.lock().unwrap() = Some(supervisor.clone());
    supervisor.start();
    wait_running(&supervisor, Duration::from_secs(30)).await;
    let first_pid = supervisor.snapshot().pid.unwrap();
    let first_credential = auth.credential().unwrap();

    let account = storage.identity().default_account_id;
    let repo = WorkerRepository::new(storage.db());
    let (worker, _) = repo
        .create_worker(account, "runtime-gate", RequestId::generate(), 1)
        .unwrap();
    let validator: Arc<dyn RuntimeValidator> = Arc::new(transport.clone());
    let controller = DeploymentController::new(
        &storage,
        artifacts.clone(),
        validator,
        BundleLimits::default(),
    );

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
    let response = dispatch(&transport, account, worker.id, &a, None, "hello-a").await;
    assert_eq!(response.status, 200);
    assert_eq!(response.loader_outcome, Some(LoaderOutcome::Cold));
    assert!(response.body.contains("A:hello-a:production:gate-secret"));
    assert!(response.body.ends_with(":API_TOKEN,MODE"));

    // Warm path is still descriptor-resolved and produces the same immutable code.
    let warm = dispatch(&transport, account, worker.id, &a, None, "warm").await;
    assert_eq!(warm.status, 200);
    assert_eq!(warm.loader_outcome, Some(LoaderOutcome::Warm));
    assert!(warm.body.contains("A:warm:production:gate-secret"));

    let named = dispatch(&transport, account, worker.id, &a, Some("Named"), "named").await;
    assert_eq!(named.status, 200);
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
    let egress = deploy_egress(&controller, account, worker.id, egress_fixture.as_ref()).await;
    let denied = dispatch(&transport, account, worker.id, &egress, None, "").await;
    assert_eq!(denied.status, 200);
    let egress_result: serde_json::Value = serde_json::from_str(&denied.body).unwrap();
    let expected_denied = if egress_fixture.is_some() { 11 } else { 9 };
    assert_eq!(egress_result["denied"], expected_denied);
    let allowed = egress_result["allowed"].as_array().unwrap();
    assert_eq!(allowed.len(), egress_fixture.as_ref().map_or(0, |_| 3));
    assert!(allowed.iter().all(|value| value == "fixture-ok"));
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
    api_matrix(
        storage.clone(),
        artifacts.clone(),
        transport.clone(),
        account,
    )
    .await;

    // Once response headers/body have started, a runtime crash truncates the
    // stream. platformd must not rewrite or replay it as a clean JSON error.
    let crash_pid = supervisor.snapshot().pid.unwrap();
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

    // A warm WorkerLoader entry must not bypass the pre-get source/descriptor
    // check. Corrupting the authority after warm load fails closed instead of
    // executing the already-cached isolate.
    let artifact = ArtifactRef::new(1, &hex::encode(a.artifact_sha256), a.artifact_size).unwrap();
    mock.corrupt_body(&artifact.physical_key("system/"));
    let warm_corrupt = dispatch(&transport, account, worker.id, &a, None, "must-not-run").await;
    assert_eq!(warm_corrupt.status, 500);
    assert!(warm_corrupt.body.contains("ARTIFACT_INTEGRITY_ERROR"));

    supervisor.shutdown().await;
    let _ = shutdown_tx.send(true);
    source_task.await.unwrap().unwrap();
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
    let mut source = String::from(
        r#"
export default {
  async fetch() {
    const publicTargets = "#,
    );
    source.push_str(&serde_json::to_string(&public_targets).unwrap());
    source.push_str(";\n    const deniedTargets = ");
    source.push_str(&serde_json::to_string(&denied_targets).unwrap());
    source.push_str(
        r#";
    const allowed = [];
    for (const target of publicTargets) {
      const response = await fetch(target, { signal: AbortSignal.timeout(3000) });
      if (!response.ok) throw new Error("public fixture status " + response.status);
      allowed.push(await response.text());
    }
    let denied = 0;
    for (const target of deniedTargets) {
      try { await fetch(target, { signal: AbortSignal.timeout(1000) }); }
      catch { denied++; }
    }
    return Response.json({ allowed, denied });
  }
};
"#,
    );
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
    let request = CreateDeploymentRequest {
        account_id: account,
        worker_id: worker,
        idempotency_key: "deploy-egress".to_owned(),
        bundle: bundle.into_bytes().into(),
        compatibility_date: "2026-08-22".to_owned(),
        compatibility_flags: Vec::new(),
        vars: BTreeMap::new(),
        secrets: BTreeMap::new(),
        limits: serde_json::json!({"profile":"default"}),
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
}

fn egress_fixture_from_env() -> Option<EgressFixture> {
    const NAMES: [&str; 5] = [
        "OPEN_COMPUTE_EGRESS_PUBLIC_IPV4_URL",
        "OPEN_COMPUTE_EGRESS_PUBLIC_IPV6_URL",
        "OPEN_COMPUTE_EGRESS_PUBLIC_HOSTNAME_URL",
        "OPEN_COMPUTE_EGRESS_REDIRECT_PRIVATE_URL",
        "OPEN_COMPUTE_EGRESS_PRIVATE_HOSTNAME_URL",
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
    ] = values.map(|value| value.expect("all controlled egress fixture URLs must be set"));
    Some(EgressFixture {
        public_ipv4_url,
        public_ipv6_url,
        public_hostname_url,
        redirect_private_url,
        private_hostname_url,
    })
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
        bundle: bundle.into_bytes().into(),
        compatibility_date: "2026-08-22".to_owned(),
        compatibility_flags: vec!["nodejs_compat".to_owned()],
        vars: BTreeMap::new(),
        secrets: BTreeMap::new(),
        limits: serde_json::json!({"profile":"default"}),
        promote: false,
        request_id: RequestId::generate(),
        now_ms: 21,
    };
    match controller.create_deployment(request).await.unwrap() {
        CreateDeploymentOutcome::Applied(result) => result.deployment,
        CreateDeploymentOutcome::Replay(_) => panic!("unexpected replay"),
    }
}

async fn api_matrix(
    storage: Arc<PlatformStorage>,
    artifacts: ArtifactStore,
    transport: WorkerdTransport,
    account: open_compute_core::AccountId,
) {
    let health = HealthCoordinator::new();
    for component in [
        ComponentName::Process,
        ComponentName::DataDir,
        ComponentName::ControlDb,
        ComponentName::MasterKey,
        ComponentName::S3,
        ComponentName::Cache,
        ComponentName::Runtime,
    ] {
        health
            .set_component(
                component,
                ComponentState::Healthy,
                Some(ReadinessReason::Ready),
            )
            .unwrap();
    }
    let metrics =
        Arc::new(MetricsRegistry::new(&MetricsConfig::default(), "gate", "gate").unwrap());
    let state = HttpState::new(
        health,
        metrics,
        false,
        &ServerConfig::default(),
        Arc::new(|| None),
    )
    .unwrap()
    .with_worker_api(WorkerApiState::new(
        storage.clone(),
        artifacts,
        transport,
        DeploymentPins::new(),
        BundleLimits::default(),
        Duration::from_secs(5),
    ));
    let app = merged_router(state);
    let create = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/v1/accounts/{account}/workers"))
                .header(header::CONTENT_TYPE, "application/json")
                .header("idempotency-key", "api-worker-create")
                .body(Body::from(r#"{"name":"api-gate"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(create.status(), 201);
    let create_json: serde_json::Value =
        serde_json::from_slice(&to_bytes(create.into_body(), 64 * 1024).await.unwrap()).unwrap();
    let worker_id = create_json["worker"]["id"].as_str().unwrap();

    let replay_create = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/v1/accounts/{account}/workers"))
                .header(header::CONTENT_TYPE, "application/json")
                .header("idempotency-key", "api-worker-create")
                .body(Body::from(r#"{"name":"api-gate"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(replay_create.status(), 201);
    let list_workers = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/v1/accounts/{account}/workers"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(list_workers.status(), 200);
    let get_worker = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/v1/accounts/{account}/workers/{worker_id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(get_worker.status(), 200);
    let missing_key = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/v1/accounts/{account}/workers"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"name":"missing-key"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(missing_key.status(), 400);

    let bundle = CanonicalBundle::build(
        "index.js",
        vec![ModuleInput {
            name: "index.js".to_owned(),
            module_type: ModuleType::EsModule,
            bytes: br#"import { WorkerEntrypoint } from "cloudflare:workers";
export class Named extends WorkerEntrypoint {
  async fetch() { return new Response("api:named"); }
}
export default { fetch(request, env) { return new Response('api:' + env.MODE); } };"#
                .to_vec(),
        }],
        BundleLimits::default(),
    )
    .unwrap();
    let metadata = serde_json::json!({
        "mainModule": "index.js",
        "compatibilityDate": "2026-08-22",
        "compatibilityFlags": [],
        "vars": {"MODE": "real"},
        "secrets": {},
        "limits": {"profile": "default"},
        "promote": true
    })
    .to_string();
    let bundle_chunks = bundle
        .into_bytes()
        .chunks(7)
        .map(|chunk| Ok::<Bytes, Infallible>(Bytes::copy_from_slice(chunk)))
        .collect::<Vec<_>>();
    let deployment = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/v1/accounts/{account}/workers/{worker_id}/deployments"
                ))
                .header(header::CONTENT_TYPE, "application/octet-stream")
                .header("idempotency-key", "api-deploy-create")
                .header("x-open-compute-deployment-metadata", metadata)
                .body(Body::from_stream(futures::stream::iter(bundle_chunks)))
                .unwrap(),
        )
        .await
        .unwrap();
    let deployment_status = deployment.status();
    let deployment_body = to_bytes(deployment.into_body(), 128 * 1024).await.unwrap();
    assert_eq!(
        deployment_status,
        201,
        "deployment response={}",
        String::from_utf8_lossy(&deployment_body)
    );
    let deployment_json: serde_json::Value = serde_json::from_slice(&deployment_body).unwrap();
    let deployment_id = deployment_json["deployment"]["id"].as_str().unwrap();
    let get_deployment = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/v1/accounts/{account}/workers/{worker_id}/deployments/{deployment_id}"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(get_deployment.status(), 200);

    let named_route = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/v1/accounts/{account}/workers/{worker_id}/routes"
                ))
                .header(header::CONTENT_TYPE, "application/json")
                .header("idempotency-key", "api-route-named")
                .body(Body::from(
                    r#"{"hostname":"named.example.test","pathPrefix":"/named","entrypoint":"Named"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let named_route_status = named_route.status();
    let named_route_body = to_bytes(named_route.into_body(), 64 * 1024).await.unwrap();
    assert_eq!(
        named_route_status,
        201,
        "named route response={}",
        String::from_utf8_lossy(&named_route_body)
    );
    let named_route_json: serde_json::Value = serde_json::from_slice(&named_route_body).unwrap();
    let named_route_id = named_route_json["route"]["id"].as_str().unwrap();
    let missing_route = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/v1/accounts/{account}/workers/{worker_id}/routes"
                ))
                .header(header::CONTENT_TYPE, "application/json")
                .header("idempotency-key", "api-route-missing")
                .body(Body::from(
                    r#"{"hostname":"named.example.test","pathPrefix":"/missing","entrypoint":"Missing"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(missing_route.status(), 404);

    let named_public = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/named/hello")
                .header(header::HOST, "named.example.test")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(named_public.status(), 200);
    assert_eq!(
        to_bytes(named_public.into_body(), 1024).await.unwrap(),
        "api:named"
    );

    let list_routes = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/v1/accounts/{account}/workers/{worker_id}/routes"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(list_routes.status(), 200);
    let delete_named_route = app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!(
                    "/v1/accounts/{account}/workers/{worker_id}/routes/{named_route_id}"
                ))
                .header("idempotency-key", "api-route-delete")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(delete_named_route.status(), 202);

    let public = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/__workers/{account}/api-gate/hello"))
                .header(header::HOST, "public.example.test")
                .header("x-open-compute-deployment-id", "forged")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(public.status(), 200);
    assert_eq!(
        to_bytes(public.into_body(), 1024).await.unwrap(),
        "api:real"
    );

    let disposable_bundle = CanonicalBundle::build(
        "index.js",
        vec![ModuleInput {
            name: "index.js".to_owned(),
            module_type: ModuleType::EsModule,
            bytes: b"export default { fetch() { return new Response('disposable'); } };".to_vec(),
        }],
        BundleLimits::default(),
    )
    .unwrap();
    let disposable_metadata = serde_json::json!({
        "mainModule": "index.js",
        "compatibilityDate": "2026-08-22",
        "compatibilityFlags": [],
        "vars": {},
        "secrets": {},
        "limits": {"profile": "default"},
        "promote": false
    })
    .to_string();
    let disposable = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/v1/accounts/{account}/workers/{worker_id}/deployments"
                ))
                .header(header::CONTENT_TYPE, "application/octet-stream")
                .header("idempotency-key", "api-deploy-disposable")
                .header("x-open-compute-deployment-metadata", disposable_metadata)
                .body(Body::from(disposable_bundle.into_bytes()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(disposable.status(), 201);
    let disposable_json: serde_json::Value =
        serde_json::from_slice(&to_bytes(disposable.into_body(), 128 * 1024).await.unwrap())
            .unwrap();
    let disposable_id = disposable_json["deployment"]["id"].as_str().unwrap();
    let promoted = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/v1/accounts/{account}/workers/{worker_id}/promotions"
                ))
                .header(header::CONTENT_TYPE, "application/json")
                .header("idempotency-key", "api-promote-disposable")
                .body(Body::from(
                    serde_json::json!({
                        "targetDeploymentId": disposable_id,
                        "expectedActiveDeploymentId": deployment_id,
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(promoted.status(), 200);
    let rolled_back = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/v1/accounts/{account}/workers/{worker_id}/rollbacks"
                ))
                .header(header::CONTENT_TYPE, "application/json")
                .header("idempotency-key", "api-rollback-original")
                .body(Body::from(
                    serde_json::json!({
                        "targetDeploymentId": deployment_id,
                        "expectedActiveDeploymentId": disposable_id,
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(rolled_back.status(), 200);
    let referenced_delete = app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!(
                    "/v1/accounts/{account}/workers/{worker_id}/deployments/{disposable_id}"
                ))
                .header("idempotency-key", "api-delete-referenced")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(referenced_delete.status(), 409);
    let _ = WorkerRepository::new(storage.db())
        .prune_expired_idempotency(i64::MAX, 1_000)
        .unwrap();
    let delete_request = || {
        Request::builder()
            .method("DELETE")
            .uri(format!(
                "/v1/accounts/{account}/workers/{worker_id}/deployments/{disposable_id}"
            ))
            .header("idempotency-key", "api-delete-complete")
            .body(Body::empty())
            .unwrap()
    };
    let deleted = app.clone().oneshot(delete_request()).await.unwrap();
    assert_eq!(deleted.status(), 202);
    let deleted_body = to_bytes(deleted.into_body(), 64 * 1024).await.unwrap();
    let replay = app.clone().oneshot(delete_request()).await.unwrap();
    assert_eq!(replay.status(), 202);
    assert_eq!(
        to_bytes(replay.into_body(), 64 * 1024).await.unwrap(),
        deleted_body
    );

    let list = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!(
                    "/v1/accounts/{account}/workers/{worker_id}/deployments"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(list.status(), 200);

    let disposable_worker = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/v1/accounts/{account}/workers"))
                .header(header::CONTENT_TYPE, "application/json")
                .header("idempotency-key", "api-disposable-worker")
                .body(Body::from(r#"{"name":"api-disposable"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(disposable_worker.status(), 201);
    let disposable_worker_json: serde_json::Value = serde_json::from_slice(
        &to_bytes(disposable_worker.into_body(), 64 * 1024)
            .await
            .unwrap(),
    )
    .unwrap();
    let disposable_worker_id = disposable_worker_json["worker"]["id"].as_str().unwrap();
    let deleted_worker = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!(
                    "/v1/accounts/{account}/workers/{disposable_worker_id}"
                ))
                .header("idempotency-key", "api-disposable-worker-delete")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(deleted_worker.status(), 202);
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
        bundle: bundle.into_bytes().into(),
        compatibility_date: "2026-08-22".to_owned(),
        compatibility_flags: vec!["rpc".to_owned()],
        vars,
        secrets,
        limits: serde_json::json!({"profile":"default"}),
        promote,
        request_id: RequestId::generate(),
        now_ms: 2,
    }
}

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

fn runtime_config(binary: PathBuf, lock: PathBuf, assets: PathBuf) -> RuntimeConfig {
    RuntimeConfig {
        binary,
        lock_file: lock,
        assets_dir: assets,
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
