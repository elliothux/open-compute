//! Real pinned-workerd P0.4 KV compatibility and persistence Gate.
//! The cohesive matrix intentionally shares one runtime generation, two
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
use open_compute_storage::{DeploymentRecord, PlatformStorage, WorkerRepository};
use open_compute_workers::{
    BundleLimits, CanonicalBundle, CreateDeploymentOutcome, CreateDeploymentRequest,
    CreateResourceOutcome, CreateResourceRequest, DeploymentBindingInput, DeploymentController,
    KvResourceDriver, ModuleInput, ModuleType, ResourceController, ResourcePins, RuntimeSource,
    RuntimeValidator,
};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn p0_4_real_kv_matrix() {
    let Some(workerd) = std::env::var_os("OPEN_COMPUTE_TEST_WORKERD").map(PathBuf::from) else {
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
    let supervisor = Arc::new(WorkerdSupervisor::new_with_services_and_auth(
        WorkerdSupervisorOptions {
            runtime,
            compiler,
            config: runtime_config(workerd, lock, root.join("runtime")),
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
    let repository = WorkerRepository::new(storage.db());
    let (worker, _) = repository
        .create_worker(account, "kv-gate", RequestId::generate(), 12)
        .unwrap();
    let validator: Arc<dyn RuntimeValidator> = Arc::new(transport.clone());
    let deployments =
        DeploymentController::new(&storage, artifacts, validator, BundleLimits::default());
    let deployment = deploy(
        &deployments,
        deployment_request(account, worker.id, primary, secondary),
    )
    .await;

    let seeded = dispatch(&transport, account, worker.id, &deployment, "/seed", "").await;
    assert_eq!((seeded.status, seeded.body.as_str()), (200, "seeded"));
    let large = dispatch(&transport, account, worker.id, &deployment, "/large", "").await;
    assert_eq!(
        (large.status, large.body.as_str()),
        (200, "26214400:7:7:true")
    );
    let cancelled = dispatch(&transport, account, worker.id, &deployment, "/cancel", "").await;
    assert_eq!(
        (cancelled.status, cancelled.body.as_str()),
        (200, "cancelled")
    );
    let snapshot = dispatch(&transport, account, worker.id, &deployment, "/snapshot", "").await;
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
    assert_eq!(value["binary"], serde_json::json!([255, 1]));
    assert_eq!(value["stream"], "stream-value");
    assert_eq!(value["other"], "isolated");
    assert_eq!(
        value["many"],
        serde_json::json!([["text", "hello"], ["missing", null]])
    );

    let first = dispatch(&transport, account, worker.id, &deployment, "/page1", "").await;
    let first: serde_json::Value = serde_json::from_str(&first.body).unwrap();
    assert_eq!(first["list_complete"], false);
    let cursor = first["cursor"].as_str().unwrap();
    let second = dispatch(
        &transport,
        account,
        worker.id,
        &deployment,
        "/page2",
        cursor,
    )
    .await;
    let second: serde_json::Value = serde_json::from_str(&second.body).unwrap();
    assert_ne!(first["keys"][0]["name"], second["keys"][0]["name"]);
    let tampered = dispatch(
        &transport,
        account,
        worker.id,
        &deployment,
        "/page2",
        &format!("{cursor}x"),
    )
    .await;
    assert_eq!(tampered.status, 500);
    assert!(tampered.body.contains("KV_CURSOR_INVALID"));

    let old_pid = supervisor.snapshot().pid.unwrap();
    supervisor.report_unhealthy();
    wait_pid_change(&supervisor, old_pid, Duration::from_secs(30)).await;
    let after_restart = dispatch(
        &transport,
        account,
        worker.id,
        &deployment,
        "/page2",
        cursor,
    )
    .await;
    assert_eq!(after_restart.status, 200, "{}", after_restart.body);
    let persisted = dispatch(&transport, account, worker.id, &deployment, "/snapshot", "").await;
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&persisted.body).unwrap()["text"],
        "hello"
    );

    let deleted = dispatch(&transport, account, worker.id, &deployment, "/delete", "").await;
    assert_eq!(deleted.body, "deleted");
    let missing = dispatch(&transport, account, worker.id, &deployment, "/missing", "").await;
    assert_eq!(missing.body, "null");
    assert_eq!(pins.count(primary), 0);
    assert_eq!(pins.count(secondary), 0);
    let write_staging = storage.data_dir().root().join("kv/.staging-write");
    assert!(std::fs::read_dir(write_staging).unwrap().next().is_none());

    supervisor.shutdown().await;
    assert_eq!(supervisor.owner_registry_len(), 0);
    let _ = shutdown_tx.send(true);
    source_task.await.unwrap().unwrap();
    binding_task.await.unwrap().unwrap();
    assert_eq!(pins.count(primary), 0);
    assert_eq!(pins.count(secondary), 0);
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
    controller: &DeploymentController<'_>,
    request: CreateDeploymentRequest,
) -> DeploymentRecord {
    match controller.create_deployment(request).await.unwrap() {
        CreateDeploymentOutcome::Applied(result) => result.deployment,
        CreateDeploymentOutcome::Replay(_) => panic!("unexpected deployment replay"),
    }
}

fn deployment_request(
    account_id: AccountId,
    worker_id: open_compute_core::WorkerId,
    primary: ResourceId,
    secondary: ResourceId,
) -> CreateDeploymentRequest {
    let source = r#"export default {
  async fetch(request, env) {
    const path = new URL(request.url).pathname;
    if (path === "/seed") {
      await env.CACHE.put("text", "hello", { metadata: { z: 2, a: 1 } });
      await env.CACHE.put("json", JSON.stringify({ ok: true }), { expirationTtl: 60 });
      const view = new Uint8Array([9, 255, 1, 8]).subarray(1, 3);
      await env.CACHE.put("binary", view);
      await env.CACHE.put("stream", new ReadableStream({
        start(controller) {
          controller.enqueue(new TextEncoder().encode("stream-"));
          controller.enqueue(new TextEncoder().encode("value"));
          controller.close();
        }
      }));
      await env.OTHER.put("text", "isolated");
      return new Response("seeded");
    }
    if (path === "/snapshot") {
      try {
        const withMetadata = await env.CACHE.getWithMetadata("text");
        const binary = Array.from(new Uint8Array(await env.CACHE.get("binary", "arrayBuffer")));
        const stream = await new Response(await env.CACHE.get("stream", "stream")).text();
        const many = Array.from((await env.CACHE.get(["text", "missing", "text"])).entries());
        return Response.json({
          text: withMetadata.value,
          metadata: withMetadata.metadata,
          json: await env.CACHE.get("json", "json"),
          binary,
          stream,
          other: await env.OTHER.get("text"),
          many,
        });
      } catch (error) {
        return new Response(String(error && error.stack ? error.stack : error), { status: 599 });
      }
    }
    if (path === "/large") {
      const streamOf = (bytes) => {
        let emitted = 0;
        return new ReadableStream({
          pull(controller) {
            if (emitted >= bytes) { controller.close(); return; }
            const size = Math.min(1024 * 1024, bytes - emitted);
            const chunk = new Uint8Array(size);
            chunk.fill(7);
            emitted += size;
            controller.enqueue(chunk);
          }
        });
      };
      const limit = 25 * 1024 * 1024;
      await env.CACHE.put("large", streamOf(limit));
      let rejected = false;
      try { await env.CACHE.put("large", streamOf(limit + 1)); } catch { rejected = true; }
      const reader = (await env.CACHE.get("large", "stream")).getReader();
      let total = 0;
      let first = null;
      let last = null;
      for (;;) {
        const next = await reader.read();
        if (next.done) break;
        if (first === null && next.value.byteLength) first = next.value[0];
        if (next.value.byteLength) last = next.value[next.value.byteLength - 1];
        total += next.value.byteLength;
      }
      return new Response(`${total}:${first}:${last}:${rejected}`);
    }
    if (path === "/cancel") {
      const reader = (await env.CACHE.get("large", "stream")).getReader();
      const first = await reader.read();
      if (first.done || first.value.byteLength === 0) throw new Error("empty stream");
      await reader.cancel("tenant cancelled");
      return new Response("cancelled");
    }
    if (path === "/page1") return Response.json(await env.CACHE.list({ limit: 1 }));
    if (path === "/page2") return Response.json(await env.CACHE.list({ limit: 1, cursor: await request.text() }));
    if (path === "/delete") { await env.CACHE.delete("text"); return new Response("deleted"); }
    if (path === "/missing") return new Response(String(await env.CACHE.get("text")));
    return new Response("missing", { status: 404 });
  }
};"#;
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
    for (name, id) in [("CACHE", primary), ("OTHER", secondary)] {
        bindings.insert(
            name.to_owned(),
            DeploymentBindingInput {
                kind: BindingKind::KvNamespace,
                id,
                permissions: CanonicalPermissions::default(),
                config: CanonicalBindingConfig::default(),
            },
        );
    }
    CreateDeploymentRequest {
        account_id,
        worker_id,
        idempotency_key: "kv-deployment".to_owned(),
        bundle: bundle.into_bytes().into(),
        compatibility_date: "2026-08-22".to_owned(),
        compatibility_flags: vec!["rpc".to_owned()],
        vars: BTreeMap::new(),
        secrets: BTreeMap::new(),
        bindings,
        limits: serde_json::json!({"profile":"default"}),
        promote: true,
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
    deployment: &DeploymentRecord,
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
                deployment_id: deployment.id,
                worker_code_sha256: hex::encode(deployment.worker_code_sha256),
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

fn runtime_config(binary: PathBuf, lock: PathBuf, assets: PathBuf) -> RuntimeConfig {
    let mut config = PlatformConfig::default().runtime;
    config.binary = binary;
    config.lock_file = lock;
    config.assets_dir = assets;
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
