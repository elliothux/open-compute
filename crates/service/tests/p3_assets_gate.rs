//! Real pinned-workerd P3.1 immutable static-assets product gate.

use axum::body::{Body, to_bytes};
use axum::http::{Method, Request, StatusCode, header};
use axum::response::Response;
use bytes::Bytes;
use futures::stream;
use open_compute_artifacts::{
    ArtifactCache, ArtifactStore, MapEnv, MockS3, ObjectBackend, resolve_s3_credentials_with,
};
use open_compute_core::{
    CacheConfig, DataConfig, ErrorCode, PlatformConfig, Redactor, RequestId, RuntimeConfig,
    StartupId, SystemClock,
};
use open_compute_runtime::{
    DirectoryServicePath, ExternalServiceAddress, GenerationAuthRegistry, OsJitter,
    PlatformReleaseMeta, StaticConfigCompiler, SupervisorState, WorkerdSupervisor,
    WorkerdSupervisorOptions, verify_runtime_binary,
};
use open_compute_service::asset_backend::AssetBindingService;
use open_compute_service::runtime_bridge::{
    DispatchTarget, WorkerdTransport, bind_runtime_source, serve_runtime_source,
};
use open_compute_service::service_invocations::ServiceInvocationRegistry;
use open_compute_service::{
    SqliteKvBindingExecutor, bind_binding_backend, serve_binding_backend_with_assets,
};
use open_compute_storage::{PlatformStorage, WorkerRepository};
use open_compute_workers::{
    AssetEntryV1, AssetHeaderOperation, AssetHeaderRule, AssetManifestV1, AssetRedirectRule,
    AssetRoutingConfigV1, BundleLimits, CanonicalBundle, CreateVersionOutcome,
    CreateVersionRequest, HtmlHandling, ModuleInput, ModuleType, NotFoundHandling, ResourcePins,
    RunWorkerFirst, RuntimeSource, RuntimeValidator, VersionAssets, VersionContent,
    VersionController, VersionPins,
};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const WORKER_SOURCE: &str = r#"
export default {
  async fetch(request, env) {
    const url = new URL(request.url);
    if (url.pathname === "/binding") {
      return env.ASSETS.fetch("https://assets.example.test/static.txt");
    }
    if (url.pathname === "/binding-shape") {
      return new Response(typeof env.ASSETS.fetch === "function"
        && typeof env.ASSETS.fetchAsset === "undefined" ? "facade" : "leaked");
    }
    return new Response(`worker:${url.pathname}`, { headers: { "x-worker": "1" } });
  }
};
"#;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn p3_assets_real_runtime_routing_binding_immutability_and_lifecycle() {
    let workerd = std::env::var_os("OPEN_COMPUTE_TEST_WORKERD")
        .map(PathBuf::from)
        .expect("OPEN_COMPUTE_TEST_WORKERD must name the verified stock runtime");
    let root = repo_root();
    let lock = root.join("packages/runtime/workerd.lock.json");
    let runtime_assets = root.join("packages/runtime");
    let temp = tempfile::tempdir().unwrap();
    let storage = Arc::new(
        PlatformStorage::bootstrap(&storage_config(&temp.path().join("data")), &SystemClock)
            .unwrap(),
    );
    let mock = MockS3::spawn("open-compute").await;
    let artifacts = artifact_store(&mock);
    let cache = Arc::new(
        ArtifactCache::open(
            storage.data_dir().artifact_cache_dir(),
            CacheConfig::default(),
            StartupId::generate(),
        )
        .unwrap(),
    );
    let runtime = verify_runtime_binary(&lock, &workerd, Duration::from_secs(10), &Redactor::new())
        .await
        .expect("formal pinned runtime");

    let source_auth = GenerationAuthRegistry::new();
    let binding_auth = GenerationAuthRegistry::new();
    let source_listener = bind_runtime_source().await.unwrap();
    let source_addr = source_listener.local_addr().unwrap();
    let binding_listener = bind_binding_backend().await.unwrap();
    let binding_addr = binding_listener.local_addr().unwrap();
    let version_pins = VersionPins::new();
    let (shutdown, mut source_shutdown) = tokio::sync::watch::channel(false);
    let mut binding_shutdown = shutdown.subscribe();
    let source_task = tokio::spawn({
        let source =
            RuntimeSource::new(storage.clone(), artifacts.clone(), BundleLimits::default());
        let auth = source_auth.clone();
        async move {
            serve_runtime_source(source_listener, source, auth, async move {
                let _ = source_shutdown.changed().await;
            })
            .await
        }
    });
    let binding_task = tokio::spawn({
        let storage = storage.clone();
        let auth = binding_auth.clone();
        let pins = version_pins.clone();
        let asset_service = Arc::new(AssetBindingService::new(
            storage.clone(),
            artifacts.clone(),
            cache,
            pins.clone(),
        ));
        let service_invocations = Arc::new(ServiceInvocationRegistry::new(storage.clone(), pins));
        async move {
            serve_binding_backend_with_assets(
                binding_listener,
                storage.clone(),
                auth,
                ResourcePins::new(),
                Arc::new(SqliteKvBindingExecutor::new(
                    storage.clone(),
                    Arc::new(SystemClock),
                )),
                None,
                None,
                None,
                open_compute_core::DurableObjectsConfig::default(),
                open_compute_core::QueuesConfig::default(),
                open_compute_core::WorkflowsConfig::default(),
                None,
                asset_service,
                service_invocations,
                None,
                None,
                async move {
                    let _ = binding_shutdown.changed().await;
                },
            )
            .await
        }
    });

    let compiler = StaticConfigCompiler::new(
        runtime.clone(),
        lock,
        runtime_assets,
        storage.data_dir().runtime_dir(),
        PlatformReleaseMeta {
            version: "p3-assets-gate".to_owned(),
        },
        Duration::from_secs(20),
        Redactor::new(),
    )
    .with_generation_auth(source_auth.clone())
    .with_binding_generation_auth(binding_auth.clone());
    let supervisor_slot = Arc::new(Mutex::new(None));
    let transport = WorkerdTransport::new(source_auth.clone(), supervisor_slot.clone())
        .with_version_pins(version_pins.clone());
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
            lease_path: Some(
                storage
                    .data_dir()
                    .runtime_dir()
                    .join("p3-assets-gate.lease"),
            ),
        },
        vec![
            ExternalServiceAddress::loopback("runtime-source", source_addr).unwrap(),
            ExternalServiceAddress::loopback("binding-backend", binding_addr).unwrap(),
            ExternalServiceAddress::loopback("observability-backend", binding_addr).unwrap(),
        ],
        vec![DirectoryServicePath::local("do-storage", &do_storage).unwrap()],
        vec![source_auth, binding_auth],
    ));
    *supervisor_slot.lock().unwrap() = Some(supervisor.clone());
    supervisor.start();
    wait_running(&supervisor, Duration::from_secs(30)).await;

    let account = storage.identity().default_account_id;
    let repo = WorkerRepository::new(storage.db());
    let (static_worker, _) = repo
        .create_worker(account, "static-site", RequestId::generate(), 1, 1_000_000)
        .unwrap();
    let validator: Arc<dyn RuntimeValidator> = Arc::new(transport.clone());
    let controller = VersionController::new(
        &storage,
        artifacts.clone(),
        validator,
        BundleLimits::default(),
    );

    let static_assets = assets(
        &artifacts,
        vec![
            (
                "/404.html",
                b"missing".as_slice(),
                "text/html; charset=utf-8",
            ),
            (
                "/index.html",
                b"static-home".as_slice(),
                "text/html; charset=utf-8",
            ),
        ],
        AssetRoutingConfigV1 {
            schema_version: 1,
            binding: None,
            run_worker_first: RunWorkerFirst::All(false),
            html_handling: HtmlHandling::AutoTrailingSlash,
            not_found_handling: NotFoundHandling::Page404,
            headers: vec![AssetHeaderRule {
                pattern: "/*".to_owned(),
                operations: vec![AssetHeaderOperation {
                    name: "x-static-gate".to_owned(),
                    value: Some("p3".to_owned()),
                }],
            }],
            redirects: vec![AssetRedirectRule {
                from: "/old".to_owned(),
                to: "/".to_owned(),
                status: 302,
            }],
        },
    )
    .await;
    let static_version = deploy(
        &controller,
        version_request(
            account,
            static_worker.id,
            "static-assets",
            VersionContent::AssetsOnly {
                assets: static_assets,
            },
            true,
            10,
        ),
    )
    .await;

    let home = dispatch(
        &transport,
        dispatch_target(account, static_worker.id, &static_version),
        Method::GET,
        "/",
        None,
    )
    .await;
    assert_eq!(home.status(), StatusCode::OK);
    assert_eq!(
        home.headers()[header::CONTENT_TYPE],
        "text/html; charset=utf-8"
    );
    assert_eq!(home.headers()["x-static-gate"], "p3");
    let etag = home.headers()[header::ETAG].to_str().unwrap().to_owned();
    assert_eq!(response_body(home).await.as_ref(), b"static-home");

    let not_modified = dispatch(
        &transport,
        dispatch_target(account, static_worker.id, &static_version),
        Method::GET,
        "/",
        Some((header::IF_NONE_MATCH.as_str(), &etag)),
    )
    .await;
    assert_eq!(not_modified.status(), StatusCode::NOT_MODIFIED);
    assert!(response_body(not_modified).await.is_empty());
    let head = dispatch(
        &transport,
        dispatch_target(account, static_worker.id, &static_version),
        Method::HEAD,
        "/",
        None,
    )
    .await;
    assert_eq!(head.status(), StatusCode::OK);
    assert_eq!(head.headers()[header::CONTENT_LENGTH], "11");
    assert!(response_body(head).await.is_empty());
    let redirect = dispatch(
        &transport,
        dispatch_target(account, static_worker.id, &static_version),
        Method::GET,
        "/old?source=gate",
        None,
    )
    .await;
    assert_eq!(redirect.status(), StatusCode::FOUND);
    assert_eq!(redirect.headers()[header::LOCATION], "/?source=gate");
    let missing = dispatch(
        &transport,
        dispatch_target(account, static_worker.id, &static_version),
        Method::GET,
        "/absent",
        None,
    )
    .await;
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    assert_eq!(response_body(missing).await.as_ref(), b"missing");
    assert_eq!(version_pins.count(static_version.id), 0);
    version_pins
        .fence_and_wait(static_version.id, Duration::from_millis(100))
        .await
        .unwrap();
    version_pins.unfence(static_version.id);

    let (hybrid_worker, _) = repo
        .create_worker(account, "hybrid-site", RequestId::generate(), 20, 1_000_000)
        .unwrap();
    let first = deploy_hybrid(
        &controller,
        &artifacts,
        HybridVersionSpec {
            account,
            worker: hybrid_worker.id,
            key: "hybrid-v1",
            static_text: "asset-v1",
            promote: true,
            now_ms: 21,
        },
    )
    .await;
    let default_asset = dispatch(
        &transport,
        dispatch_target(account, hybrid_worker.id, &first),
        Method::GET,
        "/static.txt",
        None,
    )
    .await;
    assert_eq!(response_body(default_asset).await.as_ref(), b"asset-v1");
    assert_eq!(version_pins.count(first.id), 0);
    let worker_first = dispatch(
        &transport,
        dispatch_target(account, hybrid_worker.id, &first),
        Method::GET,
        "/api/route.txt",
        None,
    )
    .await;
    assert_eq!(worker_first.headers()["x-worker"], "1");
    assert_eq!(
        response_body(worker_first).await.as_ref(),
        b"worker:/api/route.txt"
    );
    let excluded = dispatch(
        &transport,
        dispatch_target(account, hybrid_worker.id, &first),
        Method::GET,
        "/api/docs/page.txt",
        None,
    )
    .await;
    assert_eq!(response_body(excluded).await.as_ref(), b"docs-v1");
    let binding_shape = dispatch(
        &transport,
        dispatch_target(account, hybrid_worker.id, &first),
        Method::GET,
        "/binding-shape",
        None,
    )
    .await;
    let binding_shape_status = binding_shape.status();
    let binding_shape_headers = binding_shape.headers().clone();
    let binding_shape_body = response_body(binding_shape).await;
    assert_eq!(binding_shape_status, StatusCode::OK);
    assert_eq!(
        binding_shape_body.as_ref(),
        b"facade",
        "binding shape headers: {binding_shape_headers:?}"
    );
    let binding = dispatch(
        &transport,
        dispatch_target(account, hybrid_worker.id, &first),
        Method::GET,
        "/binding",
        Some(("x-open-compute-version-id", "forged")),
    )
    .await;
    assert_eq!(
        binding.status(),
        StatusCode::OK,
        "binding response: {binding:?}"
    );
    let binding_headers = binding.headers().clone();
    let binding_body = response_body(binding).await;
    assert_eq!(
        binding_body.as_ref(),
        b"asset-v1",
        "binding headers: {binding_headers:?}"
    );
    assert_eq!(version_pins.count(first.id), 1);

    let second = deploy_hybrid(
        &controller,
        &artifacts,
        HybridVersionSpec {
            account,
            worker: hybrid_worker.id,
            key: "hybrid-v2",
            static_text: "asset-v2",
            promote: true,
            now_ms: 22,
        },
    )
    .await;
    assert_eq!(
        repo.get_worker(account, hybrid_worker.id)
            .unwrap()
            .active_version_id,
        Some(second.id)
    );
    let old_asset = dispatch(
        &transport,
        dispatch_target(account, hybrid_worker.id, &first),
        Method::GET,
        "/static.txt",
        None,
    )
    .await;
    assert_eq!(response_body(old_asset).await.as_ref(), b"asset-v1");
    let new_asset = dispatch(
        &transport,
        dispatch_target(account, hybrid_worker.id, &second),
        Method::GET,
        "/static.txt",
        None,
    )
    .await;
    assert_eq!(response_body(new_asset).await.as_ref(), b"asset-v2");
    let error = version_pins
        .fence_and_wait(first.id, Duration::from_millis(50))
        .await
        .unwrap_err();
    assert_eq!(error.code(), ErrorCode::VersionReferenced);
    version_pins.unfence(first.id);

    supervisor.shutdown().await;
    assert_eq!(supervisor.owner_registry_len(), 0);
    let _ = shutdown.send(true);
    source_task.await.unwrap().unwrap();
    binding_task.await.unwrap().unwrap();
}

struct HybridVersionSpec<'a> {
    account: open_compute_core::AccountId,
    worker: open_compute_core::WorkerId,
    key: &'a str,
    static_text: &'a str,
    promote: bool,
    now_ms: i64,
}

async fn deploy_hybrid(
    controller: &VersionController<'_>,
    artifacts: &ArtifactStore,
    spec: HybridVersionSpec<'_>,
) -> open_compute_storage::VersionRecord {
    let routing = AssetRoutingConfigV1 {
        schema_version: 1,
        binding: Some("ASSETS".to_owned()),
        run_worker_first: RunWorkerFirst::Rules(vec![
            "/api/*".to_owned(),
            "!/api/docs/*".to_owned(),
            "/binding".to_owned(),
            "/binding-shape".to_owned(),
        ]),
        html_handling: HtmlHandling::None,
        not_found_handling: NotFoundHandling::None,
        headers: Vec::new(),
        redirects: Vec::new(),
    };
    let assets = assets(
        artifacts,
        vec![
            (
                "/api/docs/page.txt",
                b"docs-v1".as_slice(),
                "text/plain; charset=utf-8",
            ),
            (
                "/api/route.txt",
                b"route-asset".as_slice(),
                "text/plain; charset=utf-8",
            ),
            (
                "/static.txt",
                spec.static_text.as_bytes(),
                "text/plain; charset=utf-8",
            ),
        ],
        routing,
    )
    .await;
    let bundle = CanonicalBundle::build(
        "index.js",
        vec![ModuleInput {
            name: "index.js".to_owned(),
            module_type: ModuleType::EsModule,
            bytes: WORKER_SOURCE.as_bytes().to_vec(),
        }],
        BundleLimits::default(),
    )
    .unwrap();
    deploy(
        controller,
        version_request(
            spec.account,
            spec.worker,
            spec.key,
            VersionContent::Worker {
                bundle: bundle.into_bytes().into(),
                assets: Some(assets),
            },
            spec.promote,
            spec.now_ms,
        ),
    )
    .await
}

async fn assets(
    artifacts: &ArtifactStore,
    files: Vec<(&str, &[u8], &str)>,
    routing: AssetRoutingConfigV1,
) -> VersionAssets {
    let mut entries = Vec::with_capacity(files.len());
    for (path, bytes, content_type) in files {
        let digest = hex::encode(Sha256::digest(bytes));
        artifacts
            .put_verified(
                stream::once(async { Ok::<Bytes, std::io::Error>(Bytes::copy_from_slice(bytes)) }),
                &digest,
                bytes.len() as u64,
            )
            .await
            .unwrap();
        entries.push(AssetEntryV1 {
            path: path.to_owned(),
            sha256: digest,
            size: bytes.len() as u64,
            content_type: content_type.to_owned(),
        });
    }
    entries.sort_by(|left, right| left.path.as_bytes().cmp(right.path.as_bytes()));
    VersionAssets {
        manifest: AssetManifestV1 {
            schema_version: 1,
            entries,
        },
        routing,
    }
}

fn version_request(
    account_id: open_compute_core::AccountId,
    worker_id: open_compute_core::WorkerId,
    key: &str,
    content: VersionContent,
    promote: bool,
    now_ms: i64,
) -> CreateVersionRequest {
    CreateVersionRequest {
        account_id,
        worker_id,
        idempotency_key: key.to_owned(),
        content,
        vars: BTreeMap::new(),
        secrets: BTreeMap::new(),
        bindings: BTreeMap::new(),
        services: BTreeMap::new(),
        runtime_features: Default::default(),
        queue_consumers: Vec::new(),
        crons: Vec::new(),
        deployment_source: promote.then_some(open_compute_storage::DeploymentSource::VersionsApi),
        request_id: RequestId::generate(),
        now_ms,
    }
}

async fn deploy(
    controller: &VersionController<'_>,
    request: CreateVersionRequest,
) -> open_compute_storage::VersionRecord {
    match controller.create_version(request).await.unwrap() {
        CreateVersionOutcome::Applied(result) => result.version,
        CreateVersionOutcome::Replay(_) => panic!("unexpected version replay"),
    }
}

fn dispatch_target(
    account_id: open_compute_core::AccountId,
    worker_id: open_compute_core::WorkerId,
    version: &open_compute_storage::VersionRecord,
) -> DispatchTarget {
    DispatchTarget {
        account_id,
        worker_id,
        version_id: version.id,
        worker_code_sha256: hex::encode(version.worker_code_sha256),
        entrypoint: None,
        route_generation: 1,
        request_id: RequestId::generate(),
    }
}

async fn dispatch(
    transport: &WorkerdTransport,
    target: DispatchTarget,
    method: Method,
    uri: &str,
    extra_header: Option<(&str, &str)>,
) -> Response {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::HOST, "assets.example.test");
    if let Some((name, value)) = extra_header {
        builder = builder.header(name, value);
    }
    transport
        .dispatch(target, builder.body(Body::empty()).unwrap())
        .await
        .unwrap()
}

async fn response_body(response: Response) -> Bytes {
    to_bytes(response.into_body(), 32 * 1024 * 1024)
        .await
        .unwrap()
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

fn storage_config(root: &Path) -> DataConfig {
    DataConfig {
        path: root.to_owned(),
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
[data]
path = "/var/lib/open-compute"
master_key_file = "/var/lib/open-compute/keys/master.key"

[storage]
backend = "s3"
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
    .object_storage
    .as_s3()
    .expect("S3 config")
    .clone();
    let env = MapEnv::new()
        .with("S3_ACCESS_KEY_ID", "AKIAEXAMPLEKEYID01")
        .with(
            "S3_SECRET_ACCESS_KEY",
            "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY",
        );
    let credentials = resolve_s3_credentials_with(&config, &env).unwrap();
    ArtifactStore::new(ObjectBackend::connect_s3(&config, &credentials, 32 * 1024 * 1024).unwrap())
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_owned()
}
