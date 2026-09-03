//! Official Cloudflare SDK 7.1.0 against the live local v4 composition.

#![cfg(feature = "test-support")]

use axum::Router;
use axum::middleware;
use open_compute_artifacts::{
    ArtifactStore, MapEnv, MockS3, R2ObjectStore, S3ArtifactClient, resolve_s3_credentials_with,
};
use open_compute_core::config::{
    D1Config, KvConfig, MetricsConfig, PlatformConfig, R2Config, RuntimeConfig, SchedulerConfig,
    SecretReference, ServerConfig, StorageConfig,
};
use open_compute_core::{
    Redactor, RequestId, SecretBytes, SystemClock, SystemSchedulerClock, VersionId,
};
use open_compute_runtime::{
    DirectoryServicePath, ExternalServiceAddress, GenerationAuthRegistry, OsJitter,
    PlatformReleaseMeta, StaticConfigCompiler, SupervisorState, WorkerdSupervisor,
    WorkerdSupervisorOptions, verify_runtime_binary,
};
use open_compute_service::http::{HttpState, merged_router};
use open_compute_service::runtime_bridge::{
    WorkerdTransport, bind_runtime_source, serve_runtime_source,
};
use open_compute_service::workers_http::WorkerApiState;
use open_compute_service::workflow_http::WorkflowApiState;
use open_compute_service::{
    D1ApiState, D1BindingService, HealthCoordinator, KvApiState, MetricsRegistry, QueueApiState,
    R2ApiState, R2BindingService, SchedulerService, SearchApiState, SqliteKvBindingExecutor,
    bind_binding_backend, serve_binding_backend,
};
use open_compute_storage::{
    NewVersion, NewVersionProducts, PlatformStorage, SchedulerStore, StoredVersionSecret,
    VersionContentKind, WorkerRepository, WorkflowRepository,
};
use open_compute_workers::{BundleLimits, ResourcePins, RuntimeSource, VersionPins};
use serde_json::Value;
use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const SDK_VERSION: &str = "7.1.0";
const TOKEN: &str = "p6-cloudflare-sdk-deployer-token";
const ADMIN_TOKEN: &str = "p6-cloudflare-sdk-admin-token";
const READ_ONLY_TOKEN: &str = "p6-cloudflare-sdk-read-only-token";
const WORKER_NAME: &str = "sdk-worker";
const UPLOADED_WORKER_NAME: &str = "sdk-uploaded-worker";
const WORKFLOW_NAME: &str = "sdk-workflow";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn official_cloudflare_sdk_matches_live_v4_router_contract() {
    let fixture = Fixture::new().await;
    let requests = Arc::new(Mutex::new(Vec::new()));
    let traced = requests.clone();
    let app = fixture.app.clone().layer(middleware::from_fn(
        move |request: axum::extract::Request, next: middleware::Next| {
            let traced = traced.clone();
            async move {
                traced.lock().unwrap().push((
                    request.method().to_string(),
                    request.uri().to_string(),
                    request
                        .headers()
                        .get(axum::http::header::CONTENT_TYPE)
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or_default()
                        .to_owned(),
                ));
                next.run(request).await
            }
        },
    ));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let sdk = fixed_cloudflare_sdk();
    let output = Command::new("bun")
        .arg("tests/live-router.mjs")
        .current_dir(repo_root().join("packages/cloudflare-extension"))
        .env(
            "OPEN_COMPUTE_V4_BASE_URL",
            format!("http://{addr}/client/v4"),
        )
        .env("OPEN_COMPUTE_V4_TOKEN", TOKEN)
        .env("OPEN_COMPUTE_V4_ACCOUNT_ID", &fixture.public_account)
        .env("OPEN_COMPUTE_CLOUDFLARE_SDK_ENTRY", sdk.join("index.mjs"))
        .env("HTTP_PROXY", "http://127.0.0.1:9")
        .env("HTTPS_PROXY", "http://127.0.0.1:9")
        .env("NO_PROXY", "127.0.0.1,localhost")
        .output()
        .expect("run official Cloudflare SDK contract");
    server.abort();
    fixture.shutdown().await;
    assert!(
        output.status.success(),
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    for bytes in [&output.stdout, &output.stderr] {
        let text = String::from_utf8_lossy(bytes);
        for secret in [TOKEN, ADMIN_TOKEN, READ_ONLY_TOKEN] {
            assert!(!text.contains(secret));
        }
        assert!(!text.contains("api.cloudflare.com"));
    }
    let trace = requests.lock().unwrap();
    assert!(
        trace
            .iter()
            .all(|(_, uri, _)| uri.starts_with("/client/v4/")),
        "SDK escaped the local v4 router: {trace:?}"
    );
    assert!(
        trace
            .iter()
            .any(|(method, uri, content_type)| method == "PUT"
                && uri.contains(&format!("/workers/scripts/{UPLOADED_WORKER_NAME}"))
                && content_type.starts_with("multipart/form-data; boundary=")),
        "official SDK transport did not send the standard Worker multipart: {trace:?}"
    );
    assert!(
        trace
            .iter()
            .any(|(method, uri, content_type)| method == "PUT"
                && uri.contains(&format!("/workers/scripts/{WORKER_NAME}"))
                && content_type == "application/javascript"),
        "fixed SDK Worker upload behavior changed: {trace:?}"
    );
}

struct Fixture {
    _temp: tempfile::TempDir,
    _mock: MockS3,
    app: Router,
    public_account: String,
    supervisor: Arc<WorkerdSupervisor>,
    shutdown: tokio::sync::watch::Sender<bool>,
    source_task: tokio::task::JoinHandle<Result<(), open_compute_core::PlatformError>>,
    binding_task: tokio::task::JoinHandle<Result<(), open_compute_core::PlatformError>>,
}

impl Fixture {
    async fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("data");
        let storage =
            Arc::new(PlatformStorage::bootstrap(&storage_config(&root), &SystemClock).unwrap());
        let mock = MockS3::spawn("open-compute").await;
        let (artifacts, objects) = stores(&mock);
        let pins = ResourcePins::new();
        let runtime_path = std::env::var_os("OPEN_COMPUTE_TEST_WORKERD")
            .map(PathBuf::from)
            .expect("OPEN_COMPUTE_TEST_WORKERD must name the verified stock runtime");
        let repo = repo_root();
        let lock = repo.join("packages/runtime/workerd.lock.json");
        let runtime = verify_runtime_binary(
            &lock,
            &runtime_path,
            Duration::from_secs(10),
            &Redactor::new(),
        )
        .await
        .expect("formal pinned runtime");
        let source_auth = GenerationAuthRegistry::new();
        let binding_auth = GenerationAuthRegistry::new();
        let source_listener = bind_runtime_source().await.unwrap();
        let source_addr = source_listener.local_addr().unwrap();
        let binding_listener = bind_binding_backend().await.unwrap();
        let binding_addr = binding_listener.local_addr().unwrap();
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
            let pins = pins.clone();
            async move {
                serve_binding_backend(
                    binding_listener,
                    storage.clone(),
                    auth,
                    pins,
                    Arc::new(SqliteKvBindingExecutor::new(storage, Arc::new(SystemClock))),
                    None,
                    None,
                    None,
                    Default::default(),
                    Default::default(),
                    Default::default(),
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
            repo.join("packages/runtime"),
            storage.data_dir().runtime_dir(),
            PlatformReleaseMeta {
                version: "p6-sdk-gate".to_owned(),
            },
            Duration::from_secs(20),
            Redactor::new(),
        )
        .with_generation_auth(source_auth.clone())
        .with_binding_generation_auth(binding_auth.clone());
        let supervisor_slot = Arc::new(Mutex::new(None));
        let transport = WorkerdTransport::new(source_auth.clone(), supervisor_slot.clone());
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
                lease_path: Some(storage.data_dir().runtime_dir().join("p6-sdk-gate.lease")),
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
        let scheduler_store = Arc::new(
            SchedulerStore::open(&storage.data_dir().ensure_scheduler_db().unwrap(), 5_000, 1)
                .unwrap(),
        );
        let scheduler = Arc::new(SchedulerService::new(
            scheduler_store.clone(),
            storage.clone(),
            transport.clone(),
            SchedulerConfig::default(),
            Default::default(),
            Arc::new(SystemSchedulerClock),
        ));
        seed_worker_and_workflow(&storage);
        let r2_config = R2Config::default();
        let r2_binding = Arc::new(
            R2BindingService::new(
                storage.clone(),
                pins.clone(),
                objects.clone(),
                r2_config.clone(),
            )
            .unwrap(),
        );
        let d1_config = D1Config::default();
        let metrics = Arc::new(
            MetricsRegistry::new(&MetricsConfig::default(), "p6-sdk", "local-v4").unwrap(),
        );
        let state = HttpState::new(
            HealthCoordinator::new(),
            metrics,
            false,
            false,
            &server_config(temp.path()),
        )
        .unwrap()
        .with_worker_api(WorkerApiState::new(
            storage.clone(),
            artifacts.clone(),
            transport.clone(),
            VersionPins::new(),
            BundleLimits::default(),
            Duration::from_secs(1),
        ))
        .with_kv_api(KvApiState::new(
            storage.clone(),
            artifacts.clone(),
            pins.clone(),
            Arc::new(SqliteKvBindingExecutor::new(
                storage.clone(),
                Arc::new(SystemClock),
            )),
            KvConfig::default(),
            1_000,
            Duration::from_secs(1),
        ))
        .with_d1_api(D1ApiState::new(
            storage.clone(),
            artifacts,
            pins.clone(),
            Arc::new(D1BindingService::new(
                storage.clone(),
                pins.clone(),
                d1_config.clone(),
            )),
            d1_config,
            1_000,
            Duration::from_secs(1),
        ))
        .with_r2_api(
            R2ApiState::new(
                storage.clone(),
                objects,
                pins.clone(),
                r2_config,
                Duration::from_secs(1),
            )
            .with_binding(r2_binding),
        )
        .with_search_api(SearchApiState::new(
            storage.clone(),
            pins,
            5_000,
            Duration::from_secs(1),
        ))
        .with_queue_api(Some(QueueApiState::new(
            storage.clone(),
            scheduler.clone(),
            32,
        )))
        .with_workflow_api(Some(WorkflowApiState::new(
            storage.clone(),
            scheduler_store,
            transport,
            Default::default(),
        )))
        .with_scheduler(Some(scheduler));
        let (state, public_account) = open_compute_service::cloudflare_v4_for_test(state, storage);
        Self {
            _temp: temp,
            _mock: mock,
            app: merged_router(state),
            public_account,
            supervisor,
            shutdown,
            source_task,
            binding_task,
        }
    }

    async fn shutdown(self) {
        self.supervisor.shutdown().await;
        let _ = self.shutdown.send(true);
        self.source_task.await.unwrap().unwrap();
        self.binding_task.await.unwrap().unwrap();
        assert!(self.supervisor.snapshot().pid.is_none());
    }
}

async fn wait_running(supervisor: &WorkerdSupervisor, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    let mut state = supervisor.subscribe();
    loop {
        let snapshot = state.borrow().clone();
        if snapshot.state == SupervisorState::Running {
            return;
        }
        assert_ne!(
            snapshot.state,
            SupervisorState::Failed,
            "supervisor failed: {snapshot:?}; diagnostics={:?}",
            supervisor.last_diagnostics()
        );
        assert!(Instant::now() < deadline, "supervisor did not become ready");
        let _ = tokio::time::timeout(Duration::from_millis(250), state.changed()).await;
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

fn seed_worker_and_workflow(storage: &PlatformStorage) {
    let account = storage.identity().default_account_id;
    let workers = WorkerRepository::new(storage.db());
    let (worker, _) = workers
        .create_worker(account, WORKER_NAME, RequestId::generate(), 1, 1_000_000)
        .unwrap();
    let version = VersionId::generate();
    let revision = uuid::Uuid::now_v7().to_string();
    let envelope = storage
        .crypto()
        .encrypt(
            &SecretBytes::new(b"sdk-secret-value".to_vec()),
            account,
            worker.id,
            version,
            "SDK_SECRET",
            &revision,
        )
        .unwrap();
    let mut secrets = BTreeMap::new();
    secrets.insert(
        "SDK_SECRET".to_owned(),
        StoredVersionSecret {
            name: "SDK_SECRET".to_owned(),
            revision_id: revision,
            envelope,
        },
    );
    workers
        .insert_staging_version(
            &NewVersion {
                id: version,
                account_id: account,
                worker_id: worker.id,
                content_kind: VersionContentKind::Worker,
                artifact_sha256: Some([1; 32]),
                artifact_size: Some(1),
                artifact_schema_version: Some(1),
                main_module: Some("index.js".into()),
                worker_code_sha256: [2; 32],
                compatibility_date: "2026-08-30".into(),
                compatibility_flags: Vec::new(),
                vars: Default::default(),
                secrets,
                request_id: RequestId::generate(),
                now_ms: 2,
            },
            &NewVersionProducts::default(),
            1_000_000,
        )
        .unwrap();
    workers.begin_validation(version).unwrap();
    workers.mark_ready(version, 3).unwrap();
    workers
        .promote(account, worker.id, version, None, RequestId::generate(), 4)
        .unwrap();
    let workflows = WorkflowRepository::new(storage.db());
    let definition = workflows
        .create_definition(account, WORKFLOW_NAME, 5)
        .unwrap();
    let workflow_version = workflows
        .stage_version(account, definition.id, version, "SdkWorkflow", 6)
        .unwrap();
    workflows
        .finish_version(
            account,
            workflow_version.target.workflow_version_id,
            true,
            7,
        )
        .unwrap();
}

fn fixed_cloudflare_sdk() -> PathBuf {
    let root = repo_root();
    let lock = std::fs::read_to_string(root.join("bun.lock")).unwrap();
    assert!(lock.contains("\"cloudflare\": [\"cloudflare@7.1.0\""));
    let prefix = format!("cloudflare@{SDK_VERSION}");
    let mut installs = std::fs::read_dir(root.join("node_modules/.bun"))
        .expect("locked Bun dependencies must already be installed")
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name().to_string_lossy().starts_with(&prefix))
        .map(|entry| entry.path().join("node_modules/cloudflare"))
        .collect::<Vec<_>>();
    installs.sort();
    installs.dedup();
    assert_eq!(installs.len(), 1, "exactly one fixed Cloudflare SDK");
    let metadata: Value =
        serde_json::from_slice(&std::fs::read(installs[0].join("package.json")).unwrap()).unwrap();
    assert_eq!(metadata["version"], SDK_VERSION);
    installs.remove(0)
}

fn stores(mock: &MockS3) -> (ArtifactStore, R2ObjectStore) {
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
r2_prefix = "tenant/r2/"
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
    let client = S3ArtifactClient::connect(&config, &credentials, 32 * 1024 * 1024).unwrap();
    (
        ArtifactStore::new(client.clone()),
        R2ObjectStore::new(client),
    )
}

fn storage_config(root: &Path) -> StorageConfig {
    StorageConfig {
        data_dir: root.to_owned(),
        master_key_file: root.join("keys/master.key"),
        master_key_env: None,
        sqlite_busy_timeout_ms: 5_000,
        free_space_soft_bytes: 1_073_741_824,
        free_space_hard_bytes: 1,
    }
}

fn server_config(root: &Path) -> ServerConfig {
    ServerConfig {
        admin_auth: token(root.join("admin.token"), ADMIN_TOKEN),
        deployer_auth: token(root.join("deployer.token"), TOKEN),
        read_only_auth: token(root.join("read-only.token"), READ_ONLY_TOKEN),
        ..ServerConfig::default()
    }
}

fn token(path: PathBuf, value: &str) -> SecretReference {
    std::fs::write(&path, format!("{value}\n")).unwrap();
    let mut permissions = std::fs::metadata(&path).unwrap().permissions();
    permissions.set_mode(0o600);
    std::fs::set_permissions(&path, permissions).unwrap();
    SecretReference {
        env: None,
        file: Some(path),
    }
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_owned()
}
