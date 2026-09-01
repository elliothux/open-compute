use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use open_compute_artifacts::{
    ArtifactStore, MapEnv, MockS3, R2ObjectStore, S3ArtifactClient, resolve_s3_credentials_with,
};
use open_compute_core::clock::SystemClock;
use open_compute_core::config::{
    D1Config, DurableObjectsConfig, KvConfig, MetricsConfig, PlatformConfig, R2Config,
    RuntimeConfig, StorageConfig,
};
use open_compute_core::{
    AccountId, BindingKind, CanonicalBindingConfig, CanonicalPermissions, Redactor, RequestId,
    ResourceId, SchedulerConfig, SchedulerPoolConfig, SchedulerPoolsConfig, SystemSchedulerClock,
    WorkerId,
};
use open_compute_runtime::{
    DirectoryServicePath, ExternalServiceAddress, GenerationAuthRegistry, OsJitter,
    PlatformReleaseMeta, StaticConfigCompiler, SupervisorState, WorkerdSupervisor,
    WorkerdSupervisorOptions, verify_runtime_binary,
};
use open_compute_service::http::{self, HttpState};
use open_compute_service::runtime_bridge::{
    DispatchTarget, WorkerdTransport, bind_runtime_source, serve_runtime_source,
};
use open_compute_service::{
    D1ApiState, D1BindingService, DoApiState, HealthCoordinator, KvApiState, MetricsRegistry,
    R2ApiState, R2BindingService, SchedulerService, SqliteKvBindingExecutor, bind_binding_backend,
    serve_binding_backend,
};
use open_compute_storage::{
    D1DatabaseRepository, D1Paths, DeploymentRecord, PlatformStorage, SchedulerStore,
};
use open_compute_workers::{
    BundleLimits, CanonicalBundle, CreateDeploymentOutcome, CreateDeploymentRequest,
    DeploymentBindingInput, DeploymentController, ModuleInput, ModuleType, ResourcePins,
    RuntimeSource,
};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::task::JoinHandle;
use tower::ServiceExt as _;

const WORKER_SOURCE: &str = include_str!("../fixtures/p0_exit_worker.js");
const P1_WORKERS: &str = include_str!("../fixtures/p1-conformance/workers.mjs");
const P1_KV: &str = include_str!("../fixtures/p1-conformance/kv.mjs");
const P1_R2: &str = include_str!("../fixtures/p1-conformance/r2.mjs");
const P1_D1: &str = include_str!("../fixtures/p1-conformance/d1.mjs");
const P1_DO: &str = include_str!("../fixtures/p1-conformance/durable-objects.mjs");
const P1_ALARMS: &str = include_str!("../fixtures/p1-conformance/alarms.mjs");
const P1_WEBSOCKET: &str = include_str!("../fixtures/p1-conformance/websocket.mjs");
const P1_ADVERSARIAL: &str = include_str!("../fixtures/p1-conformance/adversarial-values.mjs");
const P1_MALICIOUS: &str = include_str!("../fixtures/p1-conformance/malicious-worker.mjs");

#[derive(Clone, Copy)]
pub(super) struct ProductBindings {
    pub(super) kv: ResourceId,
    pub(super) kv_other: ResourceId,
    pub(super) r2: ResourceId,
    pub(super) r2_other: ResourceId,
    pub(super) d1: ResourceId,
    pub(super) d1_other: ResourceId,
    pub(super) d1_corrupt: ResourceId,
    pub(super) objects: ResourceId,
    pub(super) objects_other: ResourceId,
}

pub(super) struct DispatchResponse {
    pub(super) status: u16,
    pub(super) body: String,
}

#[derive(Debug)]
struct CapacitySample {
    operation: &'static str,
    elapsed_micros: u64,
}

static CAPACITY_SAMPLES: OnceLock<Mutex<Vec<CapacitySample>>> = OnceLock::new();

pub(super) fn reset_capacity_samples() {
    capacity_samples().lock().unwrap().clear();
}

pub(super) fn capacity_summary() -> Value {
    let samples = capacity_samples().lock().unwrap();
    let mut micros = samples
        .iter()
        .map(|sample| sample.elapsed_micros)
        .collect::<Vec<_>>();
    micros.sort_unstable();
    let mut operations = BTreeMap::<&str, u64>::new();
    for sample in samples.iter() {
        *operations.entry(sample.operation).or_default() += 1;
    }
    serde_json::json!({
        "schema_version": 1,
        "samples": micros.len(),
        "p50_ms": percentile_millis(&micros, 50),
        "p95_ms": percentile_millis(&micros, 95),
        "p99_ms": percentile_millis(&micros, 99),
        "operations": operations,
    })
}

fn capacity_samples() -> &'static Mutex<Vec<CapacitySample>> {
    CAPACITY_SAMPLES.get_or_init(|| Mutex::new(Vec::new()))
}

fn record_capacity(operation: &'static str, started: Instant) {
    capacity_samples().lock().unwrap().push(CapacitySample {
        operation,
        elapsed_micros: u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX),
    });
}

fn percentile_millis(sorted_micros: &[u64], percentile: usize) -> f64 {
    if sorted_micros.is_empty() {
        return 0.0;
    }
    let rank = sorted_micros
        .len()
        .saturating_mul(percentile)
        .div_ceil(100)
        .saturating_sub(1)
        .min(sorted_micros.len() - 1);
    sorted_micros[rank] as f64 / 1_000.0
}

fn admin_operation(uri: &str) -> &'static str {
    if uri.contains("/kv/") {
        "control_kv"
    } else if uri.contains("/r2/") {
        "control_r2"
    } else if uri.contains("/d1/") {
        "control_d1"
    } else if uri.contains("/durable-objects/") {
        "control_do"
    } else if uri.contains("/scheduler/") {
        "control_scheduler"
    } else {
        "control_workers"
    }
}

fn dispatch_operation(path: &str) -> &'static str {
    if path.starts_with("/snapshot") {
        "product_read_mixed"
    } else if path.starts_with("/websocket") {
        "do_websocket"
    } else if path.starts_with("/set-alarm") || path.starts_with("/alarm-status") {
        "scheduler_alarm"
    } else {
        "product_write_mixed"
    }
}

pub(super) struct GateStack {
    pub(super) transport: WorkerdTransport,
    pub(super) supervisor: Arc<WorkerdSupervisor>,
    pub(super) scheduler: Arc<SchedulerService>,
    pub(super) d1: Arc<D1BindingService>,
    shutdown: tokio::sync::watch::Sender<bool>,
    source_task: JoinHandle<Result<(), open_compute_core::PlatformError>>,
    binding_task: JoinHandle<Result<(), open_compute_core::PlatformError>>,
}

impl GateStack {
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn start(
        storage: Arc<PlatformStorage>,
        scheduler_store: Arc<SchedulerStore>,
        artifacts: ArtifactStore,
        objects: R2ObjectStore,
        pins: ResourcePins,
        workerd: PathBuf,
        lock: PathBuf,
        assets: PathBuf,
        release: &str,
    ) -> Self {
        let runtime =
            verify_runtime_binary(&lock, &workerd, Duration::from_secs(10), &Redactor::new())
                .await
                .expect("formal pinned runtime");
        let source_auth = GenerationAuthRegistry::new();
        let binding_auth = GenerationAuthRegistry::new();
        let source_listener = bind_runtime_source().await.unwrap();
        let source_addr = source_listener.local_addr().unwrap();
        let binding_listener = bind_binding_backend().await.unwrap();
        let binding_addr = binding_listener.local_addr().unwrap();
        let d1 = Arc::new(D1BindingService::new(
            storage.clone(),
            pins.clone(),
            d1_config(),
        ));
        let r2 = Arc::new(
            R2BindingService::new(storage.clone(), pins.clone(), objects, r2_config()).unwrap(),
        );
        let (shutdown, mut source_shutdown) = tokio::sync::watch::channel(false);
        let mut binding_shutdown = shutdown.subscribe();
        let source_task = tokio::spawn({
            let source = RuntimeSource::new(storage.clone(), artifacts, BundleLimits::default());
            let auth = source_auth.clone();
            async move {
                serve_runtime_source(source_listener, source, auth, async move {
                    let _ = source_shutdown.changed().await;
                })
                .await
            }
        });
        let binding_task = tokio::spawn({
            let backend_storage = storage.clone();
            let executor_storage = storage.clone();
            let auth = binding_auth.clone();
            let backend_pins = pins.clone();
            let d1 = d1.clone();
            let scheduler_store = scheduler_store.clone();
            async move {
                serve_binding_backend(
                    binding_listener,
                    backend_storage,
                    auth,
                    backend_pins,
                    Arc::new(SqliteKvBindingExecutor::new(
                        executor_storage,
                        Arc::new(SystemClock),
                    )),
                    None,
                    Some(r2),
                    Some(d1),
                    durable_objects_config(),
                    open_compute_core::QueuesConfig::default(),
                    open_compute_core::WorkflowsConfig::default(),
                    Some(scheduler_store),
                    async move {
                        let _ = binding_shutdown.changed().await;
                    },
                )
                .await
            }
        });
        let compiler = StaticConfigCompiler::new(
            runtime.clone(),
            lock.clone(),
            assets.clone(),
            storage.data_dir().runtime_dir(),
            PlatformReleaseMeta {
                version: release.to_owned(),
            },
            Duration::from_secs(20),
            Redactor::new(),
        )
        .with_generation_auth(source_auth.clone())
        .with_binding_generation_auth(binding_auth.clone());
        let supervisor_slot = Arc::new(Mutex::new(None));
        let transport = WorkerdTransport::new(source_auth.clone(), supervisor_slot.clone())
            .with_max_request_body(32 * 1024 * 1024);
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
                lease_path: Some(storage.data_dir().runtime_dir().join("p0-exit-gate.lease")),
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
        let scheduler_config = SchedulerConfig {
            max_in_flight: 4,
            pools: SchedulerPoolsConfig {
                alarm: SchedulerPoolConfig {
                    claim_batch: 4,
                    max_in_flight: 4,
                    ..SchedulerPoolConfig::default()
                },
                ..SchedulerPoolsConfig::default()
            },
            ..SchedulerConfig::default()
        };
        let scheduler = Arc::new(SchedulerService::new(
            scheduler_store,
            storage,
            transport.clone(),
            scheduler_config,
            open_compute_core::WorkflowsConfig::default(),
            Arc::new(SystemSchedulerClock),
        ));
        Self {
            transport,
            supervisor,
            scheduler,
            d1,
            shutdown,
            source_task,
            binding_task,
        }
    }

    pub(super) async fn stop(self) {
        self.supervisor.shutdown().await;
        assert_eq!(self.supervisor.owner_registry_len(), 0);
        let _ = self.shutdown.send(true);
        self.source_task.await.unwrap().unwrap();
        self.binding_task.await.unwrap().unwrap();
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn admin_router(
    storage: Arc<PlatformStorage>,
    artifacts: ArtifactStore,
    objects: R2ObjectStore,
    pins: ResourcePins,
    stack: &GateStack,
    scheduler_store: Arc<SchedulerStore>,
) -> Router {
    let metrics = Arc::new(
        MetricsRegistry::new(&MetricsConfig::default(), "p0-exit-gate", "pinned-workerd").unwrap(),
    );
    let state = HttpState::for_test(HealthCoordinator::new(), metrics, false, None)
        .with_kv_api(KvApiState::new(
            storage.clone(),
            artifacts.clone(),
            pins.clone(),
            kv_config(),
            1_000,
            Duration::from_secs(2),
        ))
        .with_r2_api(R2ApiState::new(
            storage.clone(),
            objects,
            pins.clone(),
            r2_config(),
            Duration::from_secs(2),
        ))
        .with_d1_api(D1ApiState::new(
            storage.clone(),
            artifacts,
            pins.clone(),
            stack.d1.clone(),
            d1_config(),
            1_000,
            Duration::from_secs(2),
        ))
        .with_do_api(
            DoApiState::new(
                storage,
                pins,
                stack.transport.clone(),
                durable_objects_config(),
                Duration::from_secs(2),
            )
            .with_scheduler(Some(scheduler_store)),
        )
        .with_scheduler(Some(stack.scheduler.clone()));
    http::admin_router(state)
}

pub(super) async fn admin_json(
    router: &Router,
    method: &str,
    uri: &str,
    body: Value,
    idempotency_key: Option<&str>,
) -> (StatusCode, Value) {
    let started = Instant::now();
    let operation = admin_operation(uri);
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(key) = idempotency_key {
        builder = builder.header("idempotency-key", key);
    }
    let response = router
        .clone()
        .oneshot(
            builder
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 2 * 1024 * 1024)
        .await
        .unwrap();
    let value = serde_json::from_slice(&bytes).unwrap_or_else(
        |_| serde_json::json!({ "raw": String::from_utf8_lossy(&bytes).into_owned() }),
    );
    record_capacity(operation, started);
    (status, value)
}

pub(super) async fn deploy(
    controller: &DeploymentController<'_>,
    request: CreateDeploymentRequest,
    supervisor: &WorkerdSupervisor,
) -> DeploymentRecord {
    let started = Instant::now();
    let result = match controller
        .create_deployment(request)
        .await
        .unwrap_or_else(|error| {
            panic!(
                "deployment failed: {error:?}; runtime={:?}; diagnostics={:?}",
                supervisor.snapshot(),
                supervisor.last_diagnostics()
            )
        }) {
        CreateDeploymentOutcome::Applied(result) => result.deployment,
        CreateDeploymentOutcome::Replay(_) => panic!("unexpected deployment replay"),
    };
    record_capacity("deployment", started);
    result
}

pub(super) fn deployment_request(
    account_id: AccountId,
    worker_id: WorkerId,
    bindings: ProductBindings,
    idempotency_key: &str,
    release: &str,
    promote: bool,
    now_ms: i64,
) -> CreateDeploymentRequest {
    let bundle = CanonicalBundle::build(
        "index.js",
        std::iter::once(("index.js", WORKER_SOURCE))
            .chain([
                ("p1-conformance/workers.mjs", P1_WORKERS),
                ("p1-conformance/kv.mjs", P1_KV),
                ("p1-conformance/r2.mjs", P1_R2),
                ("p1-conformance/d1.mjs", P1_D1),
                ("p1-conformance/durable-objects.mjs", P1_DO),
                ("p1-conformance/alarms.mjs", P1_ALARMS),
                ("p1-conformance/websocket.mjs", P1_WEBSOCKET),
                ("p1-conformance/adversarial-values.mjs", P1_ADVERSARIAL),
                ("p1-conformance/malicious-worker.mjs", P1_MALICIOUS),
            ])
            .map(|(name, source)| ModuleInput {
                name: name.to_owned(),
                module_type: ModuleType::EsModule,
                bytes: source.as_bytes().to_vec(),
            })
            .collect(),
        BundleLimits::default(),
    )
    .unwrap();
    let mut resources = BTreeMap::new();
    for (name, kind, id) in [
        ("CACHE", BindingKind::KvNamespace, bindings.kv),
        ("CACHE_OTHER", BindingKind::KvNamespace, bindings.kv_other),
        ("BUCKET", BindingKind::R2Bucket, bindings.r2),
        ("BUCKET_OTHER", BindingKind::R2Bucket, bindings.r2_other),
        ("DB", BindingKind::D1Database, bindings.d1),
        ("DB_OTHER", BindingKind::D1Database, bindings.d1_other),
        ("DB_CORRUPT", BindingKind::D1Database, bindings.d1_corrupt),
        ("OBJECTS", BindingKind::DoNamespace, bindings.objects),
        (
            "OBJECTS_OTHER",
            BindingKind::DoNamespace,
            bindings.objects_other,
        ),
    ] {
        resources.insert(
            name.to_owned(),
            DeploymentBindingInput {
                kind,
                id,
                permissions: CanonicalPermissions::default(),
                config: CanonicalBindingConfig::default(),
            },
        );
    }
    CreateDeploymentRequest {
        account_id,
        worker_id,
        idempotency_key: idempotency_key.to_owned(),
        content: open_compute_workers::DeploymentContent::Worker {
            bundle: bundle.into_bytes().into(),
            assets: None,
        },
        vars: BTreeMap::from([("RELEASE".to_owned(), serde_json::json!(release))]),
        secrets: BTreeMap::new(),
        bindings: resources,
        services: BTreeMap::new(),
        runtime_features: Default::default(),
        queue_consumers: Vec::new(),
        crons: Vec::new(),
        promote,
        request_id: RequestId::generate(),
        now_ms,
    }
}

pub(super) async fn dispatch(
    transport: &WorkerdTransport,
    account_id: AccountId,
    worker_id: WorkerId,
    deployment: &DeploymentRecord,
    route_generation: u64,
    path: &str,
) -> DispatchResponse {
    let started = Instant::now();
    let operation = dispatch_operation(path);
    let response = transport
        .dispatch(
            DispatchTarget {
                account_id,
                worker_id,
                deployment_id: deployment.id,
                worker_code_sha256: hex::encode(deployment.worker_code_sha256),
                entrypoint: None,
                route_generation: i64::try_from(route_generation).unwrap(),
                request_id: RequestId::generate(),
            },
            Request::builder()
                .method("POST")
                .uri(path)
                .header(header::HOST, "p0-exit.test")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status().as_u16();
    let bytes = to_bytes(response.into_body(), 16 * 1024 * 1024)
        .await
        .unwrap();
    let response = DispatchResponse {
        status,
        body: String::from_utf8(bytes.to_vec()).unwrap(),
    };
    record_capacity(operation, started);
    response
}

pub(super) async fn wait_pid_change(
    supervisor: &WorkerdSupervisor,
    old_pid: i32,
    timeout: Duration,
) {
    let start = Instant::now();
    loop {
        let snapshot = supervisor.snapshot();
        if snapshot.state == SupervisorState::Running && snapshot.pid != Some(old_pid) {
            return;
        }
        assert!(
            snapshot.state != SupervisorState::Failed && start.elapsed() < timeout,
            "runtime did not restart: {snapshot:?}; diagnostics={:?}",
            supervisor.last_diagnostics()
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

pub(super) fn kill_workerd(pid: i32) {
    let status = Command::new("kill")
        .args(["-KILL", &pid.to_string()])
        .status()
        .unwrap();
    assert!(status.success(), "failed to SIGKILL workerd {pid}");
}

pub(super) fn corrupt_d1(storage: &PlatformStorage, account: AccountId, resource: ResourceId) {
    let record = D1DatabaseRepository::new(storage.db())
        .get(account, resource)
        .unwrap();
    let path = D1Paths::open(storage.data_dir().root())
        .unwrap()
        .resolve_storage_key(&record.storage_key, account, resource)
        .unwrap();
    std::fs::write(path, b"not-a-sqlite-database").unwrap();
}

pub(super) fn stores(mock: &MockS3) -> (ArtifactStore, R2ObjectStore) {
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
request_timeout_ms = 5000
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
    let client = S3ArtifactClient::connect(&config, &credentials, 64 * 1024 * 1024).unwrap();
    (
        ArtifactStore::new(client.clone()),
        R2ObjectStore::new(client),
    )
}

pub(super) fn storage_config(root: &Path) -> StorageConfig {
    StorageConfig {
        data_dir: root.to_path_buf(),
        master_key_file: root.join("keys/master.key"),
        master_key_env: None,
        sqlite_busy_timeout_ms: 5_000,
        free_space_soft_bytes: 1_073_741_824,
        free_space_hard_bytes: 1,
    }
}

pub(super) fn durable_objects_config() -> DurableObjectsConfig {
    DurableObjectsConfig {
        // Product Gates exercise the DO contract, not host-volume pressure.
        // Retain a fail-closed threshold while isolating them from unrelated
        // workspace utilization; dedicated storage tests own watermark policy.
        disk_high_watermark_percent: 98,
        disk_stop_writes_percent: 99,
        ..DurableObjectsConfig::default()
    }
}

pub(super) fn open_scheduler(storage: &PlatformStorage) -> Arc<SchedulerStore> {
    let path = storage.data_dir().ensure_scheduler_db().unwrap();
    Arc::new(SchedulerStore::open(&path, 5_000, now_ms()).unwrap())
}

pub(super) fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(i64::MAX)
}

pub(super) fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .unwrap()
        .to_path_buf()
}

pub(super) fn kv_config() -> KvConfig {
    KvConfig {
        namespace_quota_bytes: 256 * 1024 * 1024,
        ..KvConfig::default()
    }
}

pub(super) fn r2_config() -> R2Config {
    R2Config {
        max_object_bytes: 8 * 1024 * 1024,
        max_staging_bytes: 16 * 1024 * 1024,
        operation_timeout_ms: 5_000,
        ..R2Config::default()
    }
}

pub(super) fn d1_config() -> D1Config {
    D1Config {
        database_quota_bytes: 256 * 1024 * 1024,
        query_timeout_ms: 5_000,
        batch_timeout_ms: 5_000,
        ..D1Config::default()
    }
}

fn runtime_config() -> RuntimeConfig {
    let mut config = PlatformConfig::default().runtime;
    config.startup_timeout_ms = 20_000;
    config.shutdown_grace_ms = 1_000;
    config.kill_timeout_ms = 2_000;
    config
}

async fn wait_running(supervisor: &WorkerdSupervisor, timeout: Duration) {
    let start = Instant::now();
    loop {
        let snapshot = supervisor.snapshot();
        if snapshot.state == SupervisorState::Running {
            return;
        }
        assert!(
            snapshot.state != SupervisorState::Failed && start.elapsed() < timeout,
            "runtime failed: {snapshot:?}; diagnostics={:?}",
            supervisor.last_diagnostics()
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}
