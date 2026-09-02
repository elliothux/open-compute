//! Real pinned-workerd P0.6 D1 facade, SQLite, and restart Gate.

#![cfg(feature = "test-support")]

use axum::body::{Body, to_bytes};
use axum::http::{Request, header};
use open_compute_artifacts::{
    ArtifactStore, MapEnv, MockS3, R2ObjectStore, S3ArtifactClient, resolve_s3_credentials_with,
};
use open_compute_core::clock::SystemClock;
use open_compute_core::config::{D1Config, PlatformConfig, R2Config, RuntimeConfig, StorageConfig};
use open_compute_core::{
    AccountId, BindingKind, CanonicalBindingConfig, CanonicalPermissions, Redactor, RequestId,
    ResourceId,
};
use open_compute_runtime::{
    DirectoryServicePath, ExternalServiceAddress, GenerationAuthRegistry, OsJitter,
    PlatformReleaseMeta, StaticConfigCompiler, SupervisorState, WorkerdSupervisor,
    WorkerdSupervisorOptions, verify_runtime_binary,
};
use open_compute_service::runtime_bridge::{
    DispatchTarget, LoaderOutcome, WorkerdTransport, bind_runtime_source, serve_runtime_source,
};
use open_compute_service::{
    D1BindingService, R2BindingService, SqliteKvBindingExecutor, bind_binding_backend,
    serve_binding_backend,
};
use open_compute_storage::{
    D1_DATABASE_SCHEMA_VERSION, PlatformStorage, R2_SCHEMA_VERSION, ReserveResourceCreate,
    ResourceCreateReservation, ResourceRepository, VersionRecord, WorkerRepository,
};
use open_compute_workers::{
    BundleLimits, CanonicalBundle, CreateVersionOutcome, CreateVersionRequest, D1ResourceDriver,
    ModuleInput, ModuleType, R2ResourceDriver, ResourceDriver, ResourcePins, RuntimeSource,
    RuntimeValidator, VersionBindingInput, VersionController,
};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn p0_6_real_d1_facade_and_backend_matrix() {
    let workerd = std::env::var_os("OPEN_COMPUTE_TEST_WORKERD")
        .map(PathBuf::from)
        .expect("OPEN_COMPUTE_TEST_WORKERD must name the verified stock runtime");
    let root = repo_root();
    let lock = root.join("packages/runtime/workerd.lock.json");
    let temp = tempfile::tempdir().unwrap();
    let storage = Arc::new(
        PlatformStorage::bootstrap(&storage_config(&temp.path().join("data")), &SystemClock)
            .unwrap(),
    );
    let mock = MockS3::spawn("open-compute").await;
    let (artifacts, objects) = stores(&mock);
    let runtime = verify_runtime_binary(&lock, &workerd, Duration::from_secs(10), &Redactor::new())
        .await
        .expect("formal pinned runtime");
    let source_auth = GenerationAuthRegistry::new();
    let binding_auth = GenerationAuthRegistry::new();
    let source_listener = bind_runtime_source().await.unwrap();
    let source_addr = source_listener.local_addr().unwrap();
    let binding_listener = bind_binding_backend().await.unwrap();
    let binding_addr = binding_listener.local_addr().unwrap();
    let pins = ResourcePins::new();
    let d1_config = D1Config {
        database_quota_bytes: 256 * 1024 * 1024,
        max_result_rows: 2,
        max_result_bytes: 1_024,
        max_vm_steps: 10_000,
        query_timeout_ms: 5_000,
        batch_timeout_ms: 5_000,
        ..D1Config::default()
    };
    let r2_config = R2Config::default();
    let d1_service = Arc::new(
        D1BindingService::new(storage.clone(), pins.clone(), d1_config.clone())
            .with_response_loss_once(),
    );
    let r2_service = Arc::new(
        R2BindingService::new(
            storage.clone(),
            pins.clone(),
            objects.clone(),
            r2_config.clone(),
        )
        .unwrap(),
    );
    let (shutdown_tx, mut source_shutdown) = tokio::sync::watch::channel(false);
    let mut binding_shutdown = shutdown_tx.subscribe();
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
    let d1_service_for_backend = d1_service.clone();
    let binding_task = tokio::spawn({
        let backend_storage = storage.clone();
        let executor_storage = storage.clone();
        let auth = binding_auth.clone();
        let pins = pins.clone();
        async move {
            serve_binding_backend(
                binding_listener,
                backend_storage,
                auth,
                pins,
                Arc::new(SqliteKvBindingExecutor::new(
                    executor_storage,
                    Arc::new(SystemClock),
                )),
                None,
                Some(r2_service),
                Some(d1_service_for_backend),
                open_compute_core::DurableObjectsConfig::default(),
                open_compute_core::QueuesConfig::default(),
                open_compute_core::WorkflowsConfig::default(),
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
        lock.clone(),
        root.join("packages/runtime"),
        storage.data_dir().runtime_dir(),
        PlatformReleaseMeta {
            version: "p0.6-gate".to_owned(),
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
            lease_path: Some(storage.data_dir().runtime_dir().join("p0-6-gate.lease")),
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

    let account = storage.identity().default_account_id;
    let database = create_database(&storage, &d1_config, account, "d1");
    let other = create_database(&storage, &d1_config, account, "d1-other");
    let bucket = create_bucket(&storage, &objects, &r2_config, account).await;
    let repository = WorkerRepository::new(storage.db());
    let validator: Arc<dyn RuntimeValidator> = Arc::new(transport.clone());
    let versions = VersionController::new(&storage, artifacts, validator, BundleLimits::default());
    let (worker, _) = repository
        .create_worker(account, "d1-matrix", RequestId::generate(), 20, 1_000_000)
        .unwrap();
    let version = deploy(
        &versions,
        version_request(
            account,
            worker.id,
            database,
            Some(other),
            Some(bucket),
            "matrix-v1",
            matrix_source(),
            21,
        ),
        &supervisor,
    )
    .await;
    let cold = dispatch(&transport, account, worker.id, &version, None, "/matrix").await;
    assert_eq!(cold.status, 200, "{}", cold.body);
    assert_eq!(cold.loader_outcome, Some(LoaderOutcome::Cold));
    let matrix: serde_json::Value = serde_json::from_str(&cold.body).unwrap();
    for gate in 1..=12 {
        assert_eq!(
            matrix[format!("df{gate:02}")],
            true,
            "DF-{gate:02}: {matrix}"
        );
    }
    assert_eq!(
        matrix["realRows"],
        serde_json::json!([[1, "one"], [2, "two"]])
    );
    assert_eq!(matrix["blob"], serde_json::json!([1, 2]));
    assert_eq!(matrix["batchRollback"], true);
    assert_eq!(matrix["execPrefix"], true);
    assert_eq!(matrix["authorizer"], true);
    assert_eq!(matrix["resultUnknown"], true);
    assert_eq!(matrix["limitMatrix"], true);

    let dump = dispatch(&transport, account, worker.id, &version, None, "/dump").await;
    assert_eq!((dump.status, dump.body.as_str()), (200, "true"));
    let session = dispatch(&transport, account, worker.id, &version, None, "/session").await;
    assert_eq!(session.status, 200, "{}", session.body);
    let session_json: serde_json::Value = serde_json::from_str(&session.body).unwrap();
    for key in [
        "before",
        "opaque",
        "afterResume",
        "invalid",
        "otherDb",
        "firstRow",
        "rawNamed",
        "rawPlain",
        "metaShape",
    ] {
        assert_eq!(session_json[key], true, "{key}: {session_json}");
    }
    let bookmark = session_json["bookmark"].as_str().unwrap().to_owned();
    assert!(!bookmark.is_empty());

    d1_service.arm_response_loss_once();
    let batch_loss = dispatch(
        &transport,
        account,
        worker.id,
        &version,
        None,
        "/batch-loss",
    )
    .await;
    assert_eq!((batch_loss.status, batch_loss.body.as_str()), (200, "true"));

    let warm = dispatch(&transport, account, worker.id, &version, None, "/count").await;
    assert_eq!((warm.status, warm.body.as_str()), (200, "2"));
    assert_eq!(warm.loader_outcome, Some(LoaderOutcome::Warm));
    let named = dispatch(
        &transport,
        account,
        worker.id,
        &version,
        Some("Named"),
        "/shape",
    )
    .await;
    assert_eq!((named.status, named.body.as_str()), (200, "named:true"));

    for (name, source, expected, now) in [
        ("d1-function", function_source(), "function:true", 30_i64),
        ("d1-class", class_source(), "class:true:true", 40_i64),
    ] {
        let (shape_worker, _) = repository
            .create_worker(account, name, RequestId::generate(), now, 1_000_000)
            .unwrap();
        let shape = deploy(
            &versions,
            version_request(
                account,
                shape_worker.id,
                database,
                None,
                None,
                name,
                source,
                now + 1,
            ),
            &supervisor,
        )
        .await;
        let response = dispatch(&transport, account, shape_worker.id, &shape, None, "/shape").await;
        assert_eq!((response.status, response.body.as_str()), (200, expected));
    }

    let old_pid = supervisor.snapshot().pid.unwrap();
    supervisor.report_unhealthy();
    wait_pid_change(&supervisor, old_pid, Duration::from_secs(30)).await;
    let restarted = dispatch(&transport, account, worker.id, &version, None, "/count").await;
    assert_eq!((restarted.status, restarted.body.as_str()), (200, "2"));
    let resumed = dispatch(
        &transport,
        account,
        worker.id,
        &version,
        None,
        &format!("/resume?b={bookmark}"),
    )
    .await;
    assert_eq!(resumed.status, 200, "{}", resumed.body);
    let resumed_json: serde_json::Value = serde_json::from_str(&resumed.body).unwrap();
    assert_eq!(resumed_json["n"], 2);
    assert!(
        resumed_json["bookmark"]
            .as_str()
            .is_some_and(|value| !value.is_empty())
    );
    assert_eq!(pins.count(database), 0);
    supervisor.shutdown().await;
    assert_eq!(supervisor.owner_registry_len(), 0);
    let _ = shutdown_tx.send(true);
    source_task.await.unwrap().unwrap();
    binding_task.await.unwrap().unwrap();
    println!("P0.6 DF-01..DF-12 facade/SQLite/restart matrix PASS");
}

fn create_database(
    storage: &PlatformStorage,
    config: &D1Config,
    account: AccountId,
    key: &str,
) -> ResourceId {
    let resource = reserve(
        storage,
        account,
        BindingKind::D1Database,
        D1_DATABASE_SCHEMA_VERSION,
        key,
    );
    D1ResourceDriver::new(storage, config.database_quota_bytes)
        .create(&resource)
        .unwrap();
    ResourceRepository::new(storage.db())
        .mark_ready(resource.id, 11)
        .unwrap();
    resource.id
}

async fn create_bucket(
    storage: &PlatformStorage,
    objects: &R2ObjectStore,
    config: &R2Config,
    account: AccountId,
) -> ResourceId {
    let resource = reserve(
        storage,
        account,
        BindingKind::R2Bucket,
        R2_SCHEMA_VERSION,
        "r2",
    );
    R2ResourceDriver::new(storage, objects.clone(), config.clone())
        .create(&resource)
        .await
        .unwrap();
    ResourceRepository::new(storage.db())
        .mark_ready(resource.id, 12)
        .unwrap();
    resource.id
}

fn reserve(
    storage: &PlatformStorage,
    account: AccountId,
    kind: BindingKind,
    schema: u32,
    key: &str,
) -> open_compute_storage::ResourceRecord {
    let fingerprint = storage.crypto().fingerprint_request(key.as_bytes());
    let resource_id = ResourceId::generate();
    let reservation = ResourceRepository::new(storage.db())
        .reserve_create(
            &ReserveResourceCreate {
                account_id: account,
                kind,
                name: &format!("gate-{key}"),
                idempotency_key: &format!("p0-6-{key}"),
                fingerprint_key_id: storage.crypto().fingerprint_key_id(),
                request_fingerprint: &fingerprint,
                resource_id,
                driver_schema_version: schema,
                request_id: RequestId::generate(),
                now_ms: 10,
                expires_at_ms: 1_000,
            },
            1_000_000,
        )
        .unwrap();
    let ResourceCreateReservation::Reserved(resource) = reservation else {
        panic!("unexpected reservation")
    };
    resource
}

async fn deploy(
    controller: &VersionController<'_>,
    request: CreateVersionRequest,
    supervisor: &WorkerdSupervisor,
) -> VersionRecord {
    match controller
        .create_version(request)
        .await
        .unwrap_or_else(|error| {
            panic!(
                "version failed: {error:?}; runtime={:?}; diagnostics={:?}",
                supervisor.snapshot(),
                supervisor.last_diagnostics()
            )
        }) {
        CreateVersionOutcome::Applied(result) => result.version,
        CreateVersionOutcome::Replay(_) => panic!("unexpected version replay"),
    }
}

#[allow(clippy::too_many_arguments)]
fn version_request(
    account_id: AccountId,
    worker_id: open_compute_core::WorkerId,
    database: ResourceId,
    other: Option<ResourceId>,
    bucket: Option<ResourceId>,
    key: &str,
    source: &str,
    now_ms: i64,
) -> CreateVersionRequest {
    let bundle = CanonicalBundle::build(
        "index.js",
        vec![ModuleInput {
            name: "index.js".to_owned(),
            module_type: ModuleType::EsModule,
            bytes: source.as_bytes().to_vec(),
        }],
        BundleLimits::default(),
    )
    .unwrap();
    let mut bindings = BTreeMap::new();
    for name in ["DB", "DB_ALIAS"] {
        bindings.insert(
            name.to_owned(),
            VersionBindingInput {
                kind: BindingKind::D1Database,
                id: database,
                permissions: CanonicalPermissions::default(),
                config: CanonicalBindingConfig::default(),
            },
        );
    }
    if let Some(other) = other {
        bindings.insert(
            "OTHER".to_owned(),
            VersionBindingInput {
                kind: BindingKind::D1Database,
                id: other,
                permissions: CanonicalPermissions::default(),
                config: CanonicalBindingConfig::default(),
            },
        );
    }
    if let Some(bucket) = bucket {
        bindings.insert(
            "BUCKET".to_owned(),
            VersionBindingInput {
                kind: BindingKind::R2Bucket,
                id: bucket,
                permissions: CanonicalPermissions::default(),
                config: CanonicalBindingConfig::default(),
            },
        );
    }
    CreateVersionRequest {
        account_id,
        worker_id,
        idempotency_key: key.to_owned(),
        content: open_compute_workers::VersionContent::Worker {
            bundle: bundle.into_bytes().into(),
            assets: None,
        },
        vars: BTreeMap::new(),
        secrets: BTreeMap::new(),
        bindings,
        services: BTreeMap::new(),
        runtime_features: Default::default(),
        queue_consumers: Vec::new(),
        crons: Vec::new(),
        deployment_source: Some(open_compute_storage::DeploymentSource::VersionsApi),
        request_id: RequestId::generate(),
        now_ms,
    }
}

fn matrix_source() -> &'static str {
    include_str!("fixtures/p0_6_d1_worker.js")
}

fn function_source() -> &'static str {
    r#"import { D1Database } from "./__open_compute__/d1/facade.js";
export default async function(request, env) {
  return new Response(`function:${env.DB instanceof D1Database && typeof env.DB.prepare === "function"}`);
}"#
}

fn class_source() -> &'static str {
    r#"import { WorkerEntrypoint } from "cloudflare:workers";
import { D1Database } from "./__open_compute__/d1/facade.js";
export default class extends WorkerEntrypoint {
  constructor(ctx, env) { super(ctx, env); this.wrapped = env.DB instanceof D1Database; }
  async fetch() { return new Response(`class:${this.wrapped}:${this.env.DB instanceof D1Database}`); }
}"#
}

struct DispatchResponse {
    status: u16,
    body: String,
    loader_outcome: Option<LoaderOutcome>,
}

async fn dispatch(
    transport: &WorkerdTransport,
    account_id: AccountId,
    worker_id: open_compute_core::WorkerId,
    version: &VersionRecord,
    entrypoint: Option<&str>,
    path: &str,
) -> DispatchResponse {
    let request = Request::builder()
        .method("POST")
        .uri(path)
        .header(header::HOST, "d1.test")
        .body(Body::empty())
        .unwrap();
    let response = transport
        .dispatch(
            DispatchTarget {
                account_id,
                worker_id,
                version_id: version.id,
                worker_code_sha256: hex::encode(version.worker_code_sha256),
                entrypoint: entrypoint.map(str::to_owned),
                route_generation: 1,
                request_id: RequestId::generate(),
            },
            request,
        )
        .await
        .unwrap();
    let status = response.status().as_u16();
    let loader_outcome = response.extensions().get::<LoaderOutcome>().copied();
    let bytes = to_bytes(response.into_body(), 8 * 1024 * 1024)
        .await
        .unwrap();
    DispatchResponse {
        status,
        body: String::from_utf8(bytes.to_vec()).unwrap(),
        loader_outcome,
    }
}

async fn wait_running(supervisor: &WorkerdSupervisor, timeout: Duration) {
    let start = Instant::now();
    loop {
        let snapshot = supervisor.snapshot();
        if snapshot.state == SupervisorState::Running {
            return;
        }
        assert!(
            start.elapsed() < timeout,
            "runtime did not become ready: {snapshot:?}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

async fn wait_pid_change(supervisor: &WorkerdSupervisor, old_pid: i32, timeout: Duration) {
    let start = Instant::now();
    loop {
        let snapshot = supervisor.snapshot();
        if snapshot.state == SupervisorState::Running && snapshot.pid != Some(old_pid) {
            return;
        }
        assert!(
            start.elapsed() < timeout,
            "runtime did not restart: {snapshot:?}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
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

fn storage_config(root: &Path) -> StorageConfig {
    StorageConfig {
        data_dir: root.to_path_buf(),
        master_key_file: root.join("keys/master.key"),
        master_key_env: None,
        sqlite_busy_timeout_ms: 5_000,
        free_space_soft_bytes: 1_073_741_824,
        free_space_hard_bytes: 1,
    }
}

fn runtime_config() -> RuntimeConfig {
    let mut config = PlatformConfig::default().runtime;
    config.startup_timeout_ms = 20_000;
    config.shutdown_grace_ms = 1_000;
    config.kill_timeout_ms = 2_000;
    config
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .unwrap()
        .to_path_buf()
}
