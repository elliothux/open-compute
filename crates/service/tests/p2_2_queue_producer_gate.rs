//! Real pinned-workerd P2.2 Queue producer, persistence, and Conditional-Go Gate.

#![cfg(feature = "test-support")]

use axum::body::{Body, to_bytes};
use axum::http::{Request, header};
use base64::Engine as _;
use open_compute_artifacts::{
    ArtifactStore, MapEnv, MockS3, S3ArtifactClient, resolve_s3_credentials_with,
};
use open_compute_core::clock::SystemClock;
use open_compute_core::config::{PlatformConfig, RuntimeConfig, StorageConfig};
use open_compute_core::{
    AccountId, BindingKind, CanonicalBindingConfig, CanonicalPermissions, DurableObjectsConfig,
    ErrorCode, QueueId, QueueMessageId, Redactor, RequestId, ResourceId,
};
use open_compute_runtime::{
    DirectoryServicePath, ExternalServiceAddress, GenerationAuthRegistry, OsJitter,
    PlatformReleaseMeta, StaticConfigCompiler, SupervisorState, WorkerdSupervisor,
    WorkerdSupervisorOptions, verify_runtime_binary,
};
use open_compute_service::runtime_bridge::{
    DispatchTarget, LoaderOutcome, QueueDispatchMessage, QueueDispatchMetadata,
    QueueDispatchRequest, WorkerdTransport, bind_runtime_source, serve_runtime_source,
};
use open_compute_service::{SqliteKvBindingExecutor, bind_binding_backend, serve_binding_backend};
use open_compute_storage::{
    DeploymentRecord, PlatformStorage, QueueConfig, QueueContentType, QueueRepository,
    SchedulerStore, WorkerRepository,
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
#[path = "p2_2_queue_producer_gate/matrix.rs"]
mod matrix;
#[path = "p2_2_queue_producer_gate/scheduler.rs"]
mod scheduler;
use matrix::{assert_persisted_frames, matrix_source, max_expiry, persisted_v8_body};

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
    assert_eq!(result["initialOldestUndefined"], true);
    assert_eq!(result["backlogCount"], 7);
    assert_eq!(result["oldestIsDate"], true);
    assert_eq!(result["bytesDetached"], true);
    assert_eq!(result["v8RoundTrip"], true);
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
    assert_eq!(named.body, "named:8:true");
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
        8
    );
    assert_persisted_frames(&storage.data_dir().scheduler_db_path());
    let v8_body = persisted_v8_body(&storage.data_dir().scheduler_db_path());
    let catalog_queue = QueueRepository::new(storage.db())
        .get(account, queue)
        .unwrap();
    let live_metrics = scheduler
        .queue_metrics(
            queue,
            catalog_queue.lifecycle_generation,
            catalog_queue.config_generation,
        )
        .unwrap();
    let consumer = transport
        .dispatch_queue(
            &DispatchTarget {
                account_id: account,
                worker_id: worker.id,
                deployment_id: deployment.id,
                worker_code_sha256: hex::encode(deployment.worker_code_sha256),
                entrypoint: None,
                route_generation: generation,
                request_id: RequestId::generate(),
            },
            &QueueDispatchRequest {
                queue_name: "events".to_owned(),
                messages: vec![
                    QueueDispatchMessage {
                        id: QueueMessageId::generate().to_string(),
                        timestamp_ms: 1_700_000_000_000,
                        attempts: 2,
                        content_type: QueueContentType::Text,
                        body_base64: base64::engine::general_purpose::STANDARD.encode("retry-me"),
                    },
                    QueueDispatchMessage {
                        id: QueueMessageId::generate().to_string(),
                        timestamp_ms: 1_700_000_000_001,
                        attempts: 1,
                        content_type: QueueContentType::V8,
                        body_base64: base64::engine::general_purpose::STANDARD.encode(&v8_body),
                    },
                ],
                metadata: QueueDispatchMetadata::from_queue_metrics(live_metrics),
            },
            Duration::from_secs(5),
        )
        .await
        .expect("native Queue consumer custom event");
    assert_eq!(consumer.outcome, "ok");
    assert_eq!(consumer.retry_messages.len(), 1);
    assert_eq!(consumer.retry_messages[0].delay_seconds, Some(4));
    assert!(consumer.ack_all);
    let thrown = transport
        .dispatch_queue(
            &DispatchTarget {
                account_id: account,
                worker_id: worker.id,
                deployment_id: deployment.id,
                worker_code_sha256: hex::encode(deployment.worker_code_sha256),
                entrypoint: None,
                route_generation: generation,
                request_id: RequestId::generate(),
            },
            &QueueDispatchRequest {
                queue_name: "events".to_owned(),
                messages: vec![QueueDispatchMessage {
                    id: QueueMessageId::generate().to_string(),
                    timestamp_ms: 1_700_000_000_002,
                    attempts: 1,
                    content_type: QueueContentType::Text,
                    body_base64: base64::engine::general_purpose::STANDARD.encode("throw"),
                }],
                metadata: Default::default(),
            },
            Duration::from_secs(5),
        )
        .await
        .expect("native Queue consumer throw");
    assert_eq!(thrown.outcome, "exception");

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
        8
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
    assert_eq!(scheduler.queue_backlog_totals().unwrap().0, 8);
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
    assert_eq!(scheduler.queue_backlog_totals().unwrap().0, 9);

    let referenced = QueueController::new(&storage, scheduler.clone())
        .delete(account, queue, 1, true, RequestId::generate(), 43)
        .unwrap_err();
    assert_eq!(referenced.code(), ErrorCode::QueueReferenced);

    let max_expiry = max_expiry(&storage.data_dir().scheduler_db_path());
    let deleted = scheduler
        .sweep_queue_retention(max_expiry.saturating_add(1), 256, 4 * 1024 * 1024)
        .unwrap();
    assert_eq!(deleted.messages, 9);
    assert_eq!(scheduler.queue_backlog_totals().unwrap(), (0, 0));
    let empty = dispatch(
        &transport,
        account,
        worker.id,
        &deployment,
        generation,
        None,
        "/metrics",
    )
    .await;
    assert_eq!(empty.status, 200, "{}", empty.body);
    let empty_metrics: serde_json::Value = serde_json::from_str(&empty.body).unwrap();
    assert_eq!(empty_metrics["backlogCount"], 0);
    assert!(empty_metrics.get("oldestMessageTimestamp").is_none());

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
            .contains("currentOutputGate")
    );
    let diagnostics = format!("{:?}", supervisor.last_diagnostics());
    assert!(!diagnostics.contains("matrix-json-body"));
    supervisor.shutdown().await;
    assert_eq!(supervisor.owner_registry_len(), 0);
    let _ = shutdown_tx.send(true);
    source_task.await.unwrap().unwrap();
    binding_task.await.unwrap().unwrap();
    println!("P2.2 producer/consumer matrix pending frozen Gate");
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

pub(crate) async fn deploy(
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
        content: open_compute_workers::DeploymentContent::Worker {
            bundle: bundle.into_bytes().into(),
            assets: None,
        },
        vars,
        secrets: BTreeMap::new(),
        bindings,
        services: BTreeMap::new(),
        runtime_features: Default::default(),
        queue_consumers: Vec::new(),
        crons: Vec::new(),
        promote: true,
        request_id: RequestId::generate(),
        now_ms,
    }
}

pub(crate) struct DispatchResponse {
    status: u16,
    body: String,
    loader_outcome: Option<LoaderOutcome>,
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn dispatch(
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

pub(crate) async fn wait_running(supervisor: &WorkerdSupervisor, timeout: Duration) {
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

pub(crate) async fn wait_pid_change(
    supervisor: &WorkerdSupervisor,
    previous: i32,
    timeout: Duration,
) {
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

pub(crate) fn runtime_config() -> RuntimeConfig {
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

pub(crate) fn storage_config(root: &Path) -> StorageConfig {
    StorageConfig {
        data_dir: root.to_owned(),
        master_key_file: root.join("keys/master.key"),
        master_key_env: None,
        sqlite_busy_timeout_ms: 5_000,
        free_space_soft_bytes: 1_073_741_824,
        free_space_hard_bytes: 268_435_456,
    }
}

pub(crate) fn artifact_store(mock: &MockS3) -> ArtifactStore {
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

pub(crate) fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_owned()
}
