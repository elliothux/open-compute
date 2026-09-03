//! Real pinned-workerd dashboard and Cloudflare v4 boundary gate.

use axum::body::to_bytes;
use axum::http::{Request, StatusCode, header};
use open_compute_artifacts::{
    ARTIFACT_KEY_VERSION, ArtifactCache, ArtifactRef, ArtifactStore, MapEnv, MockS3,
    S3ArtifactClient, resolve_s3_credentials_with,
};
use open_compute_core::config::ServerConfig;
use open_compute_core::{
    CacheConfig, PlatformConfig, Redactor, RuntimeConfig, StartupId, StorageConfig, SystemClock,
};
use open_compute_runtime::{
    DirectoryServicePath, ExternalServiceAddress, GenerationAuthRegistry, OsJitter,
    PlatformReleaseMeta, StaticConfigCompiler, SupervisorState, WorkerdSupervisor,
    WorkerdSupervisorOptions, verify_runtime_binary,
};
use open_compute_service::asset_backend::AssetBindingService;
use open_compute_service::health::HealthCoordinator;
use open_compute_service::http::{HttpState, admin_router};
use open_compute_service::metrics::MetricsRegistry;
use open_compute_service::runtime_bridge::{
    WorkerdTransport, bind_runtime_source, serve_runtime_source,
};
use open_compute_service::service_invocations::ServiceInvocationRegistry;
use open_compute_service::{
    SqliteKvBindingExecutor, bind_binding_backend, bootstrap_dashboard, embedded_dashboard_files,
    serve_binding_backend_with_assets,
};
use open_compute_storage::PlatformStorage;
use open_compute_storage::{
    SYSTEM_DASHBOARD_WORKER_NAME, SystemOwnedVersionKind, VersionAssetsRepository, WorkerRepository,
};
use open_compute_workers::{BundleLimits, ResourcePins, RuntimeSource, VersionPins};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tower::ServiceExt;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dashboard_real_runtime_serves_spa_assets_and_cloudflare_v4_api() {
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
            version: "dashboard-gate".to_owned(),
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
                    .join("dashboard-gate.lease"),
            ),
        },
        vec![
            ExternalServiceAddress::loopback("runtime-source", source_addr).unwrap(),
            ExternalServiceAddress::loopback("binding-backend", binding_addr).unwrap(),
        ],
        vec![DirectoryServicePath::local("do-storage", &do_storage).unwrap()],
        vec![source_auth, binding_auth],
    ));
    *supervisor_slot.lock().unwrap() = Some(supervisor.clone());
    supervisor.start();
    wait_running(&supervisor, Duration::from_secs(30)).await;

    let _initial_dispatch = bootstrap_dashboard(
        storage.clone(),
        artifacts.clone(),
        transport.clone(),
        storage.identity().default_account_id,
        BundleLimits::default(),
    )
    .await
    .expect("dashboard bootstrap must succeed against stock workerd");

    let account_id = storage.identity().default_account_id;
    let repo = WorkerRepository::new(storage.db());
    let system_worker = repo
        .ensure_system_dashboard_worker(account_id, open_compute_core::RequestId::generate(), 1)
        .expect("system dashboard worker");
    assert_eq!(system_worker.name, SYSTEM_DASHBOARD_WORKER_NAME);
    assert!(
        repo.list_workers(account_id)
            .expect("list workers")
            .iter()
            .all(|worker| worker.name != SYSTEM_DASHBOARD_WORKER_NAME),
        "system dashboard worker must not appear in tenant catalog"
    );
    assert_eq!(
        repo.get_tenant_worker(account_id, system_worker.id)
            .expect_err("tenant worker API must hide system worker")
            .code(),
        open_compute_core::ErrorCode::WorkerNotFound
    );

    let mut system_pin = repo
        .get_system_owned_version(SystemOwnedVersionKind::Dashboard)
        .unwrap()
        .expect("dashboard version pin");
    let version_id = system_pin.active_version_id.expect("active dashboard");
    let (digest, size) = VersionAssetsRepository::new(storage.db())
        .list_asset_blobs(version_id)
        .unwrap()
        .into_iter()
        .next()
        .expect("dashboard asset blob");
    artifacts
        .delete_unreferenced(
            &ArtifactRef::new(ARTIFACT_KEY_VERSION, &hex::encode(digest), size).unwrap(),
        )
        .await
        .unwrap();
    system_pin.assets_sha256 = [0; 32];
    repo.pin_system_owned_version(&system_pin).unwrap();
    let _replayed_dispatch = bootstrap_dashboard(
        storage.clone(),
        artifacts.clone(),
        transport.clone(),
        account_id,
        BundleLimits::default(),
    )
    .await
    .expect("dashboard replay must restore missing immutable assets");
    let dispatch = bootstrap_dashboard(
        storage.clone(),
        artifacts.clone(),
        transport.clone(),
        account_id,
        BundleLimits::default(),
    )
    .await
    .expect("unchanged dashboard bootstrap must reuse the ready version");

    let direct = dispatch
        .dispatch(
            Request::builder()
                .uri("/")
                .header(header::HOST, "localhost")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .expect("direct dashboard dispatch");
    assert_eq!(
        direct.status(),
        StatusCode::OK,
        "stock workerd must serve the dashboard SPA root"
    );

    let admin_token = write_admin_secret(&temp.path().join("admin.token"));
    let metrics = Arc::new(
        MetricsRegistry::new(
            &open_compute_core::config::MetricsConfig::default(),
            "test",
            "workerd",
        )
        .unwrap(),
    );
    let server = ServerConfig {
        admin_auth: open_compute_core::config::SecretReference {
            env: None,
            file: Some(admin_token),
        },
        ..ServerConfig::default()
    };
    let state = HttpState::new(HealthCoordinator::new(), metrics, true, true, &server)
        .expect("dashboard gate HTTP state")
        .with_platform_storage(storage.clone())
        .with_dashboard_dispatch(Arc::new(RwLock::new(Some(dispatch))));
    let router = admin_router(state);

    let home = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/operator/")
                .header(header::HOST, "localhost")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(home.status(), StatusCode::OK);
    assert_eq!(
        home.headers()
            .get(header::CONTENT_SECURITY_POLICY)
            .and_then(|v| v.to_str().ok()),
        Some(
            "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; font-src 'self'; connect-src 'self'; base-uri 'self'; form-action 'self'; frame-ancestors 'none'; object-src 'none'"
        ),
        "dashboard shell must ship strict CSP"
    );
    assert_eq!(
        home.headers()
            .get(header::X_CONTENT_TYPE_OPTIONS)
            .and_then(|v| v.to_str().ok()),
        Some("nosniff")
    );
    assert!(
        home.headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.starts_with("text/html")),
        "dashboard root must serve HTML"
    );
    let home_body = to_bytes(home.into_body(), 256 * 1024).await.unwrap();
    assert!(
        home_body
            .windows(b"<title>open-compute dashboard</title>".len())
            .any(|window| window == b"<title>open-compute dashboard</title>"),
        "dashboard root must serve the embedded SPA shell"
    );
    assert_dashboard_surface_excludes_admin_token(&home_body, "dashboard-gate-admin");

    let deep_link = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/operator/login")
                .header(header::HOST, "localhost")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(deep_link.status(), StatusCode::OK);
    assert!(
        deep_link
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.starts_with("text/html")),
        "client-side routes must fall back to the SPA shell"
    );
    let deep_link_body = to_bytes(deep_link.into_body(), 256 * 1024).await.unwrap();
    assert_dashboard_surface_excludes_admin_token(&deep_link_body, "dashboard-gate-admin");

    let asset_path = sample_asset_url_path();
    let asset = router
        .clone()
        .oneshot(
            Request::builder()
                .uri(&asset_path)
                .header(header::HOST, "localhost")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(asset.status(), StatusCode::OK, "asset path={asset_path}");
    assert!(
        asset
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| {
                value.starts_with("text/javascript") || value.starts_with("application/javascript")
            }),
        "hashed dashboard assets must be served from workerd"
    );
    let asset_body = to_bytes(asset.into_body(), 2 * 1024 * 1024).await.unwrap();
    assert_dashboard_surface_excludes_admin_token(&asset_body, "dashboard-gate-admin");

    let meta = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/client/v4/open-compute/capabilities")
                .header(header::HOST, "localhost")
                .header(header::AUTHORIZATION, "Bearer dashboard-gate-admin")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(meta.status(), StatusCode::OK);
    assert!(
        meta.headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.contains("application/json")),
        "Cloudflare v4 API must not be handled by the dashboard SPA"
    );
    let meta_body = to_bytes(meta.into_body(), 64 * 1024).await.unwrap();
    let meta_json: serde_json::Value = serde_json::from_slice(&meta_body).unwrap();
    assert_eq!(meta_json["success"], true);
    assert_eq!(meta_json["result"]["wrangler_version"], "4.127.1");
    assert_dashboard_surface_excludes_admin_token(&meta_body, "dashboard-gate-admin");

    let unauthorized = router
        .oneshot(
            Request::builder()
                .uri("/client/v4/accounts")
                .header(header::HOST, "localhost")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);

    supervisor.shutdown().await;
    let _ = shutdown.send(true);
    source_task.await.unwrap().unwrap();
    binding_task.await.unwrap().unwrap();
}

fn sample_asset_url_path() -> String {
    let (path, _) = embedded_dashboard_files()
        .iter()
        .find(|(path, _)| path.starts_with("assets/") && path.ends_with(".js"))
        .expect("embedded dashboard must include a hashed JS asset");
    format!("/operator/{path}")
}

fn assert_dashboard_surface_excludes_admin_token(body: &[u8], token: &str) {
    let rendered = String::from_utf8_lossy(body).to_ascii_lowercase();
    for forbidden in [
        token,
        "bearer ",
        "authorization:",
        "localstorage",
        "sourcemappingurl",
        ".js.map",
    ] {
        assert!(
            !rendered.contains(&forbidden.to_ascii_lowercase()),
            "dashboard surface leaked forbidden material: {forbidden}"
        );
    }
}

fn write_admin_secret(path: &Path) -> PathBuf {
    std::fs::write(path, "dashboard-gate-admin").unwrap();
    let mut permissions = std::fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o600);
    std::fs::set_permissions(path, permissions).unwrap();
    path.to_owned()
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
