//! Real pinned-workerd P2.2 Queue producer, persistence, and Conditional-Go Gate.

#![cfg(feature = "test-support")]

use axum::body::{Body, to_bytes};
use axum::http::{Request, header};
use open_compute_artifacts::{
    ArtifactStore, MapEnv, MockS3, S3ArtifactClient, resolve_s3_credentials_with,
};
use open_compute_core::clock::SystemClock;
use open_compute_core::config::{PlatformConfig, RuntimeConfig, StorageConfig};
use open_compute_core::{
    AccountId, BindingKind, CanonicalBindingConfig, CanonicalPermissions, DurableObjectsConfig,
    ErrorCode, QueueId, Redactor, RequestId, ResourceId,
};
use open_compute_runtime::{
    DirectoryServicePath, ExternalServiceAddress, GenerationAuthRegistry, OsJitter,
    PlatformReleaseMeta, StaticConfigCompiler, SupervisorState, WorkerdSupervisor,
    WorkerdSupervisorOptions, verify_runtime_binary,
};
use open_compute_service::runtime_bridge::{
    DispatchTarget, LoaderOutcome, WorkerdTransport, bind_runtime_source, serve_runtime_source,
};
use open_compute_service::{SqliteKvBindingExecutor, bind_binding_backend, serve_binding_backend};
use open_compute_storage::{
    DeploymentRecord, PlatformStorage, QueueConfig, QueueRepository, SchedulerStore,
    WorkerRepository,
};
use open_compute_workers::{
    BundleLimits, CanonicalBundle, CreateDeploymentOutcome, CreateDeploymentRequest,
    CreateQueueOutcome, CreateQueueRequest, DeploymentBindingInput, DeploymentController,
    ModuleInput, ModuleType, QueueController, ResourcePins, RuntimeSource, RuntimeValidator,
};
use rusqlite::{Connection, params};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[path = "p2_2_queue_producer_gate/commit_crash.rs"]
mod commit_crash;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn p2_2_real_queue_producer_matrix() {
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
    let scheduler =
        Arc::new(SchedulerStore::open(&storage.data_dir().scheduler_db_path(), 5_000, 1).unwrap());
    let mock = MockS3::spawn("open-compute").await;
    let artifacts = artifact_store(&mock);
    let runtime = verify_runtime_binary(&lock, &workerd, Duration::from_secs(10), &Redactor::new())
        .await
        .expect("formal pinned runtime");
    let source_auth = GenerationAuthRegistry::new();
    let binding_auth = GenerationAuthRegistry::new();
    let source_listener = bind_runtime_source().await.unwrap();
    let source_addr = source_listener.local_addr().unwrap();
    let binding_listener = bind_binding_backend().await.unwrap();
    let binding_addr = binding_listener.local_addr().unwrap();
    let resource_pins = ResourcePins::new();
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
    let binding_task = tokio::spawn({
        let backend_storage = storage.clone();
        let executor_storage = storage.clone();
        let auth = binding_auth.clone();
        let pins = resource_pins.clone();
        let scheduler = scheduler.clone();
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
                None,
                None,
                DurableObjectsConfig::default(),
                open_compute_core::QueuesConfig::default(),
                open_compute_core::WorkflowsConfig::default(),
                Some(scheduler),
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
            version: "p2.2-gate".to_owned(),
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
            lease_path: Some(storage.data_dir().runtime_dir().join("p2-2-gate.lease")),
        },
        vec![
            ExternalServiceAddress::loopback("runtime-source", source_addr).unwrap(),
            ExternalServiceAddress::loopback("binding-backend", binding_addr).unwrap(),
        ],
        vec![DirectoryServicePath::local("do-storage", &do_storage).unwrap()],
        vec![source_auth.clone(), binding_auth.clone()],
    ));
    *supervisor_slot.lock().unwrap() = Some(supervisor.clone());
    supervisor.start();
    wait_running(&supervisor, Duration::from_secs(30)).await;

    let account = storage.identity().default_account_id;
    let queue = create_queue(&storage, scheduler.clone(), account);
    let workers = WorkerRepository::new(storage.db());
    let (worker, _) = workers
        .create_worker(account, "queue-gate", RequestId::generate(), 10, 1_000_000)
        .unwrap();
    let validator: Arc<dyn RuntimeValidator> = Arc::new(transport.clone());
    let deployments =
        DeploymentController::new(&storage, artifacts, validator, BundleLimits::default());

    let collision = deployments
        .create_deployment(deployment_request(
            account,
            worker.id,
            queue,
            "collision",
            true,
            true,
            20,
        ))
        .await
        .unwrap_err();
    assert_eq!(collision.code(), ErrorCode::BindingTypeMismatch);
    let foreign = AccountId::generate();
    insert_account(storage.data_dir().control_db_path(), foreign);
    let (foreign_worker, _) = workers
        .create_worker(
            foreign,
            "foreign-queue",
            RequestId::generate(),
            21,
            1_000_000,
        )
        .unwrap();
    let cross_account = deployments
        .create_deployment(deployment_request(
            foreign,
            foreign_worker.id,
            queue,
            "cross-account",
            true,
            false,
            22,
        ))
        .await
        .unwrap_err();
    assert_eq!(cross_account.code(), ErrorCode::QueueNotFound);

    let deployment = deploy(
        &deployments,
        deployment_request(account, worker.id, queue, "queue-bound", true, false, 30),
    )
    .await;
    let generation = i64::try_from(
        workers
            .get_worker(account, worker.id)
            .unwrap()
            .route_generation,
    )
    .unwrap();
    let matrix = dispatch(
        &transport,
        account,
        worker.id,
        &deployment,
        generation,
        None,
        "/matrix",
    )
    .await;
    assert_eq!(matrix.status, 200, "{}", matrix.body);
    assert_eq!(matrix.loader_outcome, Some(LoaderOutcome::Cold));
    let result: serde_json::Value = serde_json::from_str(&matrix.body).unwrap();
    assert_eq!(result["initialCount"], 0);
    assert_eq!(result["backlogCount"], 6);
    assert_eq!(result["oldestIsDate"], true);
    assert_eq!(result["errors"], 7);

    let named = dispatch(
        &transport,
        account,
        worker.id,
        &deployment,
        generation,
        Some("Named"),
        "/",
    )
    .await;
    assert_eq!(named.status, 200, "{}", named.body);
    assert_eq!(named.body, "named:7:true");
    assert_eq!(named.loader_outcome, Some(LoaderOutcome::Cold));
    let warm = dispatch(
        &transport,
        account,
        worker.id,
        &deployment,
        generation,
        None,
        "/metrics",
    )
    .await;
    assert_eq!(warm.loader_outcome, Some(LoaderOutcome::Warm));
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&warm.body).unwrap()["backlogCount"],
        7
    );
    assert_persisted_frames(&storage.data_dir().scheduler_db_path());

    let before_restart = supervisor.snapshot().pid.unwrap();
    supervisor.report_unhealthy();
    wait_pid_change(&supervisor, before_restart, Duration::from_secs(30)).await;
    let restored = dispatch(
        &transport,
        account,
        worker.id,
        &deployment,
        generation,
        None,
        "/metrics",
    )
    .await;
    assert_eq!(restored.status, 200, "{}", restored.body);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&restored.body).unwrap()["backlogCount"],
        7
    );

    let catalog = QueueRepository::new(storage.db());
    let current = catalog.get(account, queue).unwrap();
    scheduler
        .begin_queue_config(
            queue,
            current.lifecycle_generation,
            current.config_generation,
            40,
        )
        .unwrap();
    let mut config = current.config;
    config.delivery_delay_seconds = 11;
    catalog
        .write_config_pending(account, queue, current.config_generation, config, 41)
        .unwrap();
    let fenced = dispatch(
        &transport,
        account,
        worker.id,
        &deployment,
        generation,
        None,
        "/send-one",
    )
    .await;
    assert_eq!(fenced.status, 500);
    assert!(
        fenced.body.contains("QUEUE_CONFIG_PENDING"),
        "{}",
        fenced.body
    );
    assert_eq!(scheduler.queue_backlog_totals().unwrap().0, 7);
    assert_eq!(
        QueueController::new(&storage, scheduler.clone())
            .reconcile_pending(16, 42)
            .unwrap(),
        1
    );
    let after_reconcile = dispatch(
        &transport,
        account,
        worker.id,
        &deployment,
        generation,
        None,
        "/send-one",
    )
    .await;
    assert_eq!(after_reconcile.status, 200, "{}", after_reconcile.body);
    assert_eq!(scheduler.queue_backlog_totals().unwrap().0, 8);

    let referenced = QueueController::new(&storage, scheduler.clone())
        .delete(account, queue, 1, true, RequestId::generate(), 43)
        .unwrap_err();
    assert_eq!(referenced.code(), ErrorCode::QueueReferenced);

    let max_expiry = max_expiry(&storage.data_dir().scheduler_db_path());
    let deleted = scheduler
        .sweep_queue_retention(max_expiry.saturating_add(1), 256, 4 * 1024 * 1024)
        .unwrap();
    assert_eq!(deleted.messages, 8);
    assert_eq!(scheduler.queue_backlog_totals().unwrap(), (0, 0));

    let plain = deploy(
        &deployments,
        deployment_request(account, worker.id, queue, "queue-plain", false, false, 50),
    )
    .await;
    assert_ne!(plain.id, deployment.id);
    workers
        .prune_expired_idempotency(24 * 60 * 60 * 1_000 + 100, 100)
        .unwrap();
    workers
        .begin_deployment_delete(account, worker.id, deployment.id)
        .unwrap();
    workers
        .finalize_deployment_delete(account, worker.id, deployment.id, RequestId::generate(), 51)
        .unwrap();
    let retired = QueueController::new(&storage, scheduler.clone())
        .delete(account, queue, 1, false, RequestId::generate(), 52)
        .unwrap();
    assert_eq!(retired.purged_messages, 0);

    assert!(
        include_str!("../../../packages/runtime/dist/queues/facade.js")
            .contains("QUEUE_DO_OUTPUT_GATE_UNSUPPORTED")
    );
    let diagnostics = format!("{:?}", supervisor.last_diagnostics());
    assert!(!diagnostics.contains("matrix-json-body"));
    supervisor.shutdown().await;
    assert_eq!(supervisor.owner_registry_len(), 0);
    let _ = shutdown_tx.send(true);
    source_task.await.unwrap().unwrap();
    binding_task.await.unwrap().unwrap();
    println!("QG-01..QG-10 Conditional Go; P2.2 producer matrix PASS");
}

fn create_queue(
    storage: &PlatformStorage,
    scheduler: Arc<SchedulerStore>,
    account_id: AccountId,
) -> QueueId {
    let config = QueueConfig {
        delivery_delay_seconds: 5,
        retention_seconds: 60,
        ..QueueConfig::default()
    };
    match QueueController::new(storage, scheduler)
        .create(&CreateQueueRequest {
            account_id,
            name: "events".to_owned(),
            config,
            idempotency_key: "queue-create".to_owned(),
            request_id: RequestId::generate(),
            now_ms: 1,
        })
        .unwrap()
    {
        CreateQueueOutcome::Applied(result) => result.queue.id,
        CreateQueueOutcome::Replay(_) => panic!("unexpected Queue create replay"),
    }
}

async fn deploy(
    controller: &DeploymentController<'_>,
    request: CreateDeploymentRequest,
) -> DeploymentRecord {
    match controller.create_deployment(request).await.unwrap() {
        CreateDeploymentOutcome::Applied(result) => result.deployment,
        CreateDeploymentOutcome::Replay(_) => panic!("unexpected deployment replay"),
    }
}

#[allow(clippy::too_many_arguments)]
fn deployment_request(
    account_id: AccountId,
    worker_id: open_compute_core::WorkerId,
    queue_id: QueueId,
    key: &str,
    bound: bool,
    collision: bool,
    now_ms: i64,
) -> CreateDeploymentRequest {
    let bundle = CanonicalBundle::build(
        "index.js",
        vec![ModuleInput {
            name: "index.js".to_owned(),
            module_type: ModuleType::EsModule,
            bytes: matrix_source().as_bytes().to_vec(),
        }],
        BundleLimits::default(),
    )
    .unwrap();
    let mut vars = BTreeMap::new();
    if collision {
        vars.insert("EVENTS".to_owned(), serde_json::json!("collision"));
    }
    let mut bindings = BTreeMap::new();
    if bound {
        bindings.insert(
            "EVENTS".to_owned(),
            DeploymentBindingInput {
                kind: BindingKind::QueueProducer,
                id: ResourceId::from_uuid(queue_id.as_uuid()).unwrap(),
                permissions: CanonicalPermissions::default(),
                config: CanonicalBindingConfig::default(),
            },
        );
    }
    CreateDeploymentRequest {
        account_id,
        worker_id,
        idempotency_key: key.to_owned(),
        bundle: bundle.into_bytes().into(),
        compatibility_date: "2026-08-22".to_owned(),
        compatibility_flags: vec!["rpc".to_owned()],
        vars,
        secrets: BTreeMap::new(),
        bindings,
        queue_consumers: Vec::new(),
        crons: None,
        limits: serde_json::json!({"profile":"default"}),
        promote: true,
        request_id: RequestId::generate(),
        now_ms,
    }
}

fn matrix_source() -> &'static str {
    r#"import { WorkerEntrypoint } from "cloudflare:workers";

const codeOf = (error) => String(error && (error.stableCode || error.message) || error);
const rejects = async (fn, code) => {
  try { await fn(); return false; } catch (error) { return codeOf(error).includes(code); }
};

export class Named extends WorkerEntrypoint {
  async fetch() {
    const result = await this.env.EVENTS.send("named", { contentType: "text", delaySeconds: 0 });
    return new Response(`named:${result.metadata.metrics.backlogCount}:${result.metadata.metrics.oldestMessageTimestamp instanceof Date}`);
  }
}

export default {
  async fetch(request, env) {
    const path = new URL(request.url).pathname;
    if (path === "/metrics") return Response.json(await env.EVENTS.metrics());
    if (path === "/send-one") {
      try { return Response.json(await env.EVENTS.send({ after: "reconcile" })); }
      catch (error) { return new Response(codeOf(error), { status: 500 }); }
    }
    if (path !== "/matrix") return new Response("plain");
    const initial = await env.EVENTS.metrics();
    await env.EVENTS.send({ marker: "matrix-json-body" });
    await env.EVENTS.send("héllo", { contentType: "text", delaySeconds: 0 });
    const bytes = new Uint8Array([1, 2, 3]);
    const pending = env.EVENTS.send(bytes, { contentType: "bytes", delaySeconds: 1 });
    bytes[0] = 9;
    await pending;
    function* messages() {
      yield { body: "batch-a", contentType: "text" };
      yield { body: { batch: "b" }, delaySeconds: 0 };
      yield { body: new Uint8Array([4, 5]), contentType: "bytes", delaySeconds: 9 };
    }
    const response = await env.EVENTS.sendBatch(messages(), { delaySeconds: 7 });
    const failures = [
      await rejects(() => env.EVENTS.send("x", { contentType: "v8" }), "QUEUE_CONTENT_TYPE_UNSUPPORTED"),
      await rejects(() => env.EVENTS.send(new Uint8Array(128001), { contentType: "bytes" }), "QUEUE_MESSAGE_TOO_LARGE"),
      await rejects(() => env.EVENTS.send("x", { contentType: "text", delaySeconds: 86401 }), "QUEUE_DELAY_INVALID"),
      await rejects(() => env.EVENTS.sendBatch([]), "QUEUE_INVALID_MESSAGE"),
      await rejects(() => env.EVENTS.sendBatch(Array.from({ length: 101 }, () => ({ body: 1 }))), "QUEUE_BATCH_LIMIT_EXCEEDED"),
      await rejects(() => env.EVENTS.send(undefined), "QUEUE_INVALID_MESSAGE"),
      await rejects(() => env.EVENTS.send("x", { unexpected: true }), "QUEUE_INVALID_MESSAGE"),
    ];
    return Response.json({
      initialCount: initial.backlogCount,
      backlogCount: response.metadata.metrics.backlogCount,
      oldestIsDate: response.metadata.metrics.oldestMessageTimestamp instanceof Date,
      errors: failures.filter(Boolean).length,
    });
  }
};"#
}

struct DispatchResponse {
    status: u16,
    body: String,
    loader_outcome: Option<LoaderOutcome>,
}

#[allow(clippy::too_many_arguments)]
async fn dispatch(
    transport: &WorkerdTransport,
    account_id: AccountId,
    worker_id: open_compute_core::WorkerId,
    deployment: &DeploymentRecord,
    route_generation: i64,
    entrypoint: Option<&str>,
    path: &str,
) -> DispatchResponse {
    let request = Request::builder()
        .method("POST")
        .uri(path)
        .header(header::HOST, "queue.test")
        .body(Body::empty())
        .unwrap();
    let response = transport
        .dispatch(
            DispatchTarget {
                account_id,
                worker_id,
                deployment_id: deployment.id,
                worker_code_sha256: hex::encode(deployment.worker_code_sha256),
                entrypoint: entrypoint.map(str::to_owned),
                route_generation,
                request_id: RequestId::generate(),
            },
            request,
        )
        .await
        .unwrap();
    let status = response.status().as_u16();
    let loader_outcome = response.extensions().get::<LoaderOutcome>().copied();
    let bytes = to_bytes(response.into_body(), 2 * 1024 * 1024)
        .await
        .unwrap();
    DispatchResponse {
        status,
        body: String::from_utf8(bytes.to_vec()).unwrap(),
        loader_outcome,
    }
}

fn assert_persisted_frames(path: &Path) {
    let connection = Connection::open(path).unwrap();
    let mut statement = connection
        .prepare(
            "SELECT content_type, body, available_at_ms - enqueued_at_ms
             FROM queue_messages ORDER BY seq",
        )
        .unwrap();
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Vec<u8>>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(rows.len(), 7);
    assert_eq!(
        rows[0],
        (
            "json".to_owned(),
            br#"{"marker":"matrix-json-body"}"#.to_vec(),
            5_000
        )
    );
    assert_eq!(rows[1], ("text".to_owned(), "héllo".as_bytes().to_vec(), 0));
    assert_eq!(rows[2], ("bytes".to_owned(), vec![1, 2, 3], 1_000));
    assert_eq!(rows[3].2, 7_000);
    assert_eq!(rows[4].2, 0);
    assert_eq!(rows[5].2, 9_000);
    assert_eq!(rows[6], ("text".to_owned(), b"named".to_vec(), 0));
}

fn max_expiry(path: &Path) -> i64 {
    Connection::open(path)
        .unwrap()
        .query_row("SELECT MAX(expires_at_ms) FROM queue_messages", [], |row| {
            row.get(0)
        })
        .unwrap()
}

fn insert_account(path: PathBuf, account_id: AccountId) {
    Connection::open(path)
        .unwrap()
        .execute(
            "INSERT INTO accounts (id, name, created_at_ms, deleted_at_ms)
             VALUES (?1, ?2, 1, NULL)",
            params![account_id.to_string(), format!("foreign-{account_id}")],
        )
        .unwrap();
}

async fn wait_running(supervisor: &WorkerdSupervisor, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    let mut receiver = supervisor.subscribe();
    loop {
        let snapshot = receiver.borrow().clone();
        if snapshot.state == SupervisorState::Running {
            return;
        }
        assert_ne!(snapshot.state, SupervisorState::Failed, "{snapshot:?}");
        assert!(Instant::now() < deadline, "runtime did not become ready");
        let _ = tokio::time::timeout(Duration::from_millis(250), receiver.changed()).await;
    }
}

async fn wait_pid_change(supervisor: &WorkerdSupervisor, previous: i32, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    let mut receiver = supervisor.subscribe();
    loop {
        let snapshot = receiver.borrow().clone();
        if snapshot.state == SupervisorState::Running && snapshot.pid != Some(previous) {
            return;
        }
        assert!(Instant::now() < deadline, "runtime did not restart");
        let _ = tokio::time::timeout(Duration::from_millis(250), receiver.changed()).await;
    }
}

fn runtime_config() -> RuntimeConfig {
    RuntimeConfig {
        startup_timeout_ms: 20_000,
        shutdown_grace_ms: 500,
        drain_timeout_ms: 100,
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
