//! Real pinned-workerd P0.4 KV compatibility and persistence Gate.
//! The cohesive matrix intentionally shares one runtime generation, three
//! namespaces, a restart, and a final leak audit.

#![cfg(feature = "test-support")]

use axum::body::{Body, to_bytes};
use axum::http::{Request, header};
use open_compute_artifacts::{
    ArtifactStore, MapEnv, MockS3, S3ArtifactClient, resolve_s3_credentials_with,
};
use open_compute_core::clock::SystemClock;
use open_compute_core::config::{PlatformConfig, RuntimeConfig, StorageConfig};
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
    DispatchTarget, WorkerdTransport, bind_runtime_source, serve_runtime_source,
};
use open_compute_service::{SqliteKvBindingExecutor, bind_binding_backend, serve_binding_backend};
use open_compute_storage::{PlatformStorage, VersionRecord, WorkerRepository};
use open_compute_workers::{
    BundleLimits, CanonicalBundle, CreateResourceOutcome, CreateResourceRequest,
    CreateVersionOutcome, CreateVersionRequest, KvResourceDriver, ModuleInput, ModuleType,
    ResourceController, ResourcePins, RuntimeSource, RuntimeValidator, VersionBindingInput,
    VersionController,
};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn p0_4_real_kv_matrix() {
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
    let pins = ResourcePins::new();
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
        let binding_storage = storage.clone();
        let executor_storage = storage.clone();
        let auth = binding_auth.clone();
        let pins = pins.clone();
        async move {
            serve_binding_backend(
                binding_listener,
                binding_storage,
                auth,
                pins,
                Arc::new(SqliteKvBindingExecutor::new(
                    executor_storage,
                    Arc::new(SystemClock),
                )),
                None,
                None,
                None,
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
        assets,
        storage.data_dir().runtime_dir(),
        PlatformReleaseMeta {
            version: "p0.4-gate".to_owned(),
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
            lease_path: Some(storage.data_dir().runtime_dir().join("p0-4-gate.lease")),
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
    let resources = ResourceController::new(
        &storage,
        pins.clone(),
        KvResourceDriver::new(&storage, 256 * 1024 * 1024),
    );
    let primary = create_resource(&resources, account, "primary", "create-primary", 10);
    let secondary = create_resource(&resources, account, "secondary", "create-secondary", 11);
    let readonly = create_resource(&resources, account, "readonly", "create-readonly", 12);
    let repository = WorkerRepository::new(storage.db());
    let (worker, _) = repository
        .create_worker(account, "kv-gate", RequestId::generate(), 12, 1_000_000)
        .unwrap();
    let validator: Arc<dyn RuntimeValidator> = Arc::new(transport.clone());
    let versions = VersionController::new(&storage, artifacts, validator, BundleLimits::default());
    let version = deploy(
        &versions,
        version_request(account, worker.id, primary, secondary, readonly),
    )
    .await;

    let seeded = dispatch(&transport, account, worker.id, &version, "/seed", "").await;
    assert_eq!((seeded.status, seeded.body.as_str()), (200, "seeded"));
    let large = dispatch(&transport, account, worker.id, &version, "/large", "").await;
    assert_eq!(
        (large.status, large.body.as_str()),
        (200, "26214400:7:7:true")
    );
    let cancelled = dispatch(&transport, account, worker.id, &version, "/cancel", "").await;
    assert_eq!(
        (cancelled.status, cancelled.body.as_str()),
        (200, "cancelled")
    );
    let snapshot = dispatch(&transport, account, worker.id, &version, "/snapshot", "").await;
    assert_eq!(
        snapshot.status,
        200,
        "{}; supervisor={:?}; diagnostics={:?}",
        snapshot.body,
        supervisor.snapshot(),
        supervisor.last_diagnostics()
    );
    let value: serde_json::Value = serde_json::from_str(&snapshot.body).unwrap();
    assert_eq!(value["text"], "hello");
    assert_eq!(value["json"]["ok"], true);
    assert_eq!(value["metadata"], serde_json::json!({"a": 1, "z": 2}));
    assert_eq!(value["cacheStatus"], serde_json::Value::Null);
    assert_eq!(value["typedText"], "hello");
    assert_eq!(value["typedJson"]["ok"], true);
    assert_eq!(value["optionText"], "hello");
    assert_eq!(value["binary"], serde_json::json!([255, 1]));
    assert_eq!(value["stream"], "stream-value");
    assert_eq!(value["other"], "isolated");
    assert_eq!(
        value["many"],
        serde_json::json!([["text", "hello"], ["missing", null]])
    );
    assert_eq!(value["manyMeta"][0][1]["value"], "hello");
    assert_eq!(value["manyMeta"][0][1]["metadata"]["a"], 1);
    assert!(value["manyMeta"][0][1].get("cacheStatus").is_none());
    assert_eq!(value["manyMeta"][1][1], serde_json::Value::Null);

    let first = dispatch(&transport, account, worker.id, &version, "/page1", "").await;
    let first: serde_json::Value = serde_json::from_str(&first.body).unwrap();
    assert_eq!(first["list_complete"], false);
    assert_eq!(first["cacheStatus"], serde_json::Value::Null);
    assert!(first["cursor"].is_string());
    let cursor = first["cursor"].as_str().unwrap();
    let second = dispatch(&transport, account, worker.id, &version, "/page2", cursor).await;
    let second: serde_json::Value = serde_json::from_str(&second.body).unwrap();
    assert_ne!(first["keys"][0]["name"], second["keys"][0]["name"]);
    let tampered = dispatch(
        &transport,
        account,
        worker.id,
        &version,
        "/page2",
        &format!("{cursor}x"),
    )
    .await;
    assert_eq!(tampered.status, 599);
    assert!(
        tampered.body.contains("KV GET failed: 400 Invalid cursor"),
        "{}",
        tampered.body
    );

    let complete = dispatch(
        &transport,
        account,
        worker.id,
        &version,
        "/list-complete",
        "",
    )
    .await;
    let complete: serde_json::Value = serde_json::from_str(&complete.body).unwrap();
    assert_eq!(complete["list_complete"], true);
    assert_eq!(complete["cacheStatus"], serde_json::Value::Null);
    assert!(complete.get("cursor").is_none());
    let expiring = dispatch(
        &transport,
        account,
        worker.id,
        &version,
        "/list-expiring",
        "",
    )
    .await;
    let expiring: serde_json::Value = serde_json::from_str(&expiring.body).unwrap();
    assert_eq!(expiring["name"], "expiring");
    assert_eq!(expiring["hasExpiration"], true);
    assert_eq!(expiring["list_complete"], true);
    assert_eq!(expiring["hasCursor"], false);
    assert_eq!(expiring["cacheStatus"], serde_json::Value::Null);

    let failures = dispatch(&transport, account, worker.id, &version, "/failures", "").await;
    assert_eq!(
        failures.status,
        200,
        "{}; supervisor={:?}; diagnostics={:?}",
        failures.body,
        supervisor.snapshot(),
        supervisor.last_diagnostics()
    );
    let failures: serde_json::Value = serde_json::from_str(&failures.body).unwrap();
    let assert_error = |field: &str, name: &str, message: &str| {
        assert_eq!(failures[field]["synchronous"], false, "{field}");
        assert_eq!(failures[field]["name"], name, "{field}");
        assert_eq!(failures[field]["message"], message, "{field}");
    };
    assert_error("emptyKey", "TypeError", "Key name cannot be empty.");
    assert_error("dot", "TypeError", "\".\" is not allowed as a key name.");
    assert_error(
        "dotDot",
        "TypeError",
        "\"..\" is not allowed as a key name.",
    );
    assert_error(
        "longKey",
        "Error",
        "KV GET failed: 414 UTF-8 encoded length of 513 exceeds key length limit of 512.",
    );
    assert_error(
        "utf16",
        "Error",
        "KV GET failed: 400 Could not URL-decode key name",
    );
    assert_eq!(failures["numberKey"], serde_json::Value::Null);
    assert_error(
        "emptyBulk",
        "Error",
        "KV GET_BULK failed: 400 You must request a minimum of 1 key",
    );
    assert_error(
        "emptyBulkKey",
        "Error",
        "KV GET_BULK failed: 400 Key name  is not legal",
    );
    assert_error(
        "dotBulkKey",
        "Error",
        "KV GET_BULK failed: 400 Key name . is not legal",
    );
    assert_error(
        "longBulkKey",
        "Error",
        "KV GET_BULK failed: 414 Encoded length of 513 is too long",
    );
    assert_error(
        "invalidMetadataBulkKey",
        "Error",
        "KV GET_BULK failed: 400 Key name .. is not legal",
    );
    assert_eq!(failures["utf16Bulk"], serde_json::Value::Null);
    assert_error(
        "tooMany",
        "Error",
        "KV GET_BULK failed: 400 You can request a maximum of 100 keys",
    );
    assert_error(
        "invalidType",
        "TypeError",
        "Unknown response type. Possible types are \"text\", \"arrayBuffer\", \"json\", and \"stream\".",
    );
    assert_error(
        "bulkStream",
        "Error",
        "KV GET_BULK failed: 400 \"stream\" is not a valid type. Use \"json\" or \"text\"",
    );
    assert_error(
        "cacheTtl",
        "Error",
        "KV GET failed: 400 Invalid cache_ttl of 29. Cache TTL must be at least 30.",
    );
    assert_eq!(failures["bothExpiration"], serde_json::Value::Null);
    assert_error(
        "ttlLow",
        "Error",
        "KV PUT failed: 400 Invalid expiration_ttl of 59. Expiration TTL must be at least 60.",
    );
    let invalid_value = "KV put() accepts only strings, ArrayBuffers, ArrayBufferViews, and ReadableStreams as values.";
    assert_error("objectValue", "TypeError", invalid_value);
    assert_error("detached", "TypeError", invalid_value);
    assert_eq!(failures["extraList"], serde_json::Value::Null);
    assert_eq!(failures["zeroList"], serde_json::Value::Null);
    assert_error(
        "highList",
        "Error",
        "KV GET failed: 400 Invalid key_count_limit of 1001. Please specify integer less than 1000.",
    );
    assert_eq!(failures["numberPrefix"], serde_json::Value::Null);
    assert_eq!(failures["utf16Prefix"], serde_json::Value::Null);
    assert_eq!(failures["jsonError"], "SyntaxError");
    assert_eq!(failures["rab"], serde_json::json!([9, 8, 7, 6]));
    if !failures["sab"].is_null() {
        assert_eq!(failures["sab"], serde_json::json!([1, 2, 3]));
    }
    assert_error("readOnlyPut", "Error", "BINDING_PERMISSION_DENIED");
    assert_eq!(failures["readOnlyGet"], serde_json::Value::Null);

    let old_pid = supervisor.snapshot().pid.unwrap();
    supervisor.report_unhealthy();
    wait_pid_change(&supervisor, old_pid, Duration::from_secs(30)).await;
    let after_restart = dispatch(&transport, account, worker.id, &version, "/page2", cursor).await;
    assert_eq!(after_restart.status, 200, "{}", after_restart.body);
    let persisted = dispatch(&transport, account, worker.id, &version, "/snapshot", "").await;
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&persisted.body).unwrap()["text"],
        "hello"
    );

    let deleted = dispatch(&transport, account, worker.id, &version, "/delete", "").await;
    assert_eq!(deleted.body, "deleted");
    let missing = dispatch(&transport, account, worker.id, &version, "/missing", "").await;
    assert_eq!(missing.body, "null");
    // Content-Length completion can reach the client before the blocking stream
    // producer drops its pin. Await its existing drain notification, not a sleep.
    for resource in [primary, secondary, readonly] {
        pins.fence_and_wait(resource, Duration::from_secs(1))
            .await
            .expect("completed KV operations must release their pins before shutdown");
    }
    assert_eq!(pins.count(primary), 0);
    assert_eq!(pins.count(secondary), 0);
    assert_eq!(pins.count(readonly), 0);
    let write_staging = storage.data_dir().root().join("kv/.staging-write");
    assert!(std::fs::read_dir(write_staging).unwrap().next().is_none());

    supervisor.shutdown().await;
    assert_eq!(supervisor.owner_registry_len(), 0);
    let _ = shutdown_tx.send(true);
    source_task.await.unwrap().unwrap();
    binding_task.await.unwrap().unwrap();
    assert_eq!(pins.count(primary), 0);
    assert_eq!(pins.count(secondary), 0);
    assert_eq!(pins.count(readonly), 0);
    println!("P0.4 stock-workerd CRUD/stream/list/restart matrix PASS");
}

fn create_resource(
    controller: &ResourceController<'_, KvResourceDriver<'_>>,
    account: AccountId,
    name: &str,
    key: &str,
    now_ms: i64,
) -> ResourceId {
    match controller
        .create(&CreateResourceRequest {
            account_id: account,
            kind: BindingKind::KvNamespace,
            name: name.to_owned(),
            idempotency_key: key.to_owned(),
            driver_schema_version: 1,
            request_id: RequestId::generate(),
            now_ms,
        })
        .unwrap()
    {
        CreateResourceOutcome::Applied(result) => result.resource_id,
        CreateResourceOutcome::Replay(_) => panic!("unexpected resource replay"),
    }
}

async fn deploy(
    controller: &VersionController<'_>,
    request: CreateVersionRequest,
) -> VersionRecord {
    match controller.create_version(request).await.unwrap() {
        CreateVersionOutcome::Applied(result) => result.version,
        CreateVersionOutcome::Replay(_) => panic!("unexpected version replay"),
    }
}

fn version_request(
    account_id: AccountId,
    worker_id: open_compute_core::WorkerId,
    primary: ResourceId,
    secondary: ResourceId,
    readonly: ResourceId,
) -> CreateVersionRequest {
    let bundle = CanonicalBundle::build(
        "index.js",
        vec![ModuleInput {
            name: "index.js".to_owned(),
            module_type: ModuleType::EsModule,
            bytes: include_str!("fixtures/p0_4_kv_worker.js")
                .as_bytes()
                .to_vec(),
        }],
        BundleLimits::default(),
    )
    .unwrap();
    let mut bindings = BTreeMap::new();
    for (name, id, permissions) in [
        ("CACHE", primary, CanonicalPermissions::default()),
        ("OTHER", secondary, CanonicalPermissions::default()),
        (
            "READONLY",
            readonly,
            CanonicalPermissions {
                read: true,
                write: false,
            },
        ),
    ] {
        bindings.insert(
            name.to_owned(),
            VersionBindingInput {
                kind: BindingKind::KvNamespace,
                id,
                permissions,
                config: CanonicalBindingConfig::default(),
            },
        );
    }
    CreateVersionRequest {
        account_id,
        worker_id,
        idempotency_key: "kv-version".to_owned(),
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
        now_ms: 20,
    }
}

struct DispatchResponse {
    status: u16,
    body: String,
}

async fn dispatch(
    transport: &WorkerdTransport,
    account_id: AccountId,
    worker_id: open_compute_core::WorkerId,
    version: &VersionRecord,
    path: &str,
    body: &str,
) -> DispatchResponse {
    let request = Request::builder()
        .method("POST")
        .uri(path)
        .header(header::HOST, "kv.test")
        .body(Body::from(body.to_owned()))
        .unwrap();
    let response = transport
        .dispatch(
            DispatchTarget {
                account_id,
                worker_id,
                version_id: version.id,
                worker_code_sha256: hex::encode(version.worker_code_sha256),
                entrypoint: None,
                route_generation: 1,
                request_id: RequestId::generate(),
            },
            request,
        )
        .await
        .unwrap();
    let status = response.status().as_u16();
    let bytes = to_bytes(response.into_body(), 4 * 1024 * 1024)
        .await
        .unwrap();
    DispatchResponse {
        status,
        body: String::from_utf8(bytes.to_vec()).unwrap(),
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
    ArtifactStore::new(S3ArtifactClient::connect(&config, &credentials, 64 * 1024 * 1024).unwrap())
}

fn storage_config(root: &Path) -> StorageConfig {
    StorageConfig {
        data_dir: root.to_path_buf(),
        master_key_file: root.join("keys/master.key"),
        master_key_env: None,
        sqlite_busy_timeout_ms: 5_000,
        free_space_soft_bytes: 1_073_741_824,
        free_space_hard_bytes: 268_435_456,
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
