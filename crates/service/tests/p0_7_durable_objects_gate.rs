//! Real pinned-workerd P0.7 Durable Objects identity, facet, lifecycle, and restart Gate.
//!
//! This intentionally stays one cohesive process matrix so one fixture proves identity,
//! deployment fencing, native persistence, `WebSockets`, and destructive lifecycle together.

#![cfg(feature = "test-support")]

use axum::body::{Body, to_bytes};
use axum::http::{Request, header};
use hmac::{Hmac, Mac};
use open_compute_artifacts::{
    ArtifactStore, MapEnv, MockS3, S3ArtifactClient, resolve_s3_credentials_with,
};
use open_compute_core::clock::SystemClock;
use open_compute_core::config::{PlatformConfig, RuntimeConfig, StorageConfig};
use open_compute_core::{
    AccountId, BindingKind, CanonicalBindingConfig, CanonicalPermissions, DurableObjectId,
    Redactor, RequestId, ResourceId, WorkerId,
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
use open_compute_storage::{
    DO_NAMESPACE_SCHEMA_VERSION, DeploymentRecord, DurableObjectRepository, PlatformStorage,
    WorkerRepository,
};
use open_compute_workers::{
    BundleLimits, CanonicalBundle, CreateDeploymentOutcome, CreateDeploymentRequest,
    CreateResourceOutcome, CreateResourceRequest, DeploymentBindingInput, DeploymentController,
    DurableObjectResourceDriver, ModuleInput, ModuleType, ResourceController, ResourcePins,
    RuntimeSource, RuntimeValidator,
};
use sha2::Sha256;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn p0_7_real_durable_objects_matrix() {
    let Some(workerd) = std::env::var_os("OPEN_COMPUTE_TEST_WORKERD").map(PathBuf::from) else {
        return;
    };
    let root = repo_root();
    let lock = root.join("runtime/workerd.lock.json");
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
        root.join("runtime"),
        storage.data_dir().runtime_dir(),
        PlatformReleaseMeta {
            version: "p0.7-gate".to_owned(),
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
            lease_path: Some(storage.data_dir().runtime_dir().join("p0-7-gate.lease")),
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
    let workers = WorkerRepository::new(storage.db());
    let (worker, _) = workers
        .create_worker(account, "do-matrix", RequestId::generate(), 10)
        .unwrap();
    let counter = create_namespace(
        &storage,
        resource_pins.clone(),
        account,
        worker.id,
        "Counter",
        "counter",
        11,
    );
    let other = create_namespace(
        &storage,
        resource_pins.clone(),
        account,
        worker.id,
        "OtherCounter",
        "other",
        12,
    );
    let validator: Arc<dyn RuntimeValidator> = Arc::new(transport.clone());
    let deployments =
        DeploymentController::new(&storage, artifacts, validator, BundleLimits::default());
    let deployment_a = deploy(
        &deployments,
        deployment_request(
            account, worker.id, counter, other, "deploy-a", "A", 20, true,
        ),
        &supervisor,
    )
    .await;
    let generation_a = workers
        .get_worker(account, worker.id)
        .unwrap()
        .route_generation;

    let ids = dispatch(
        &transport,
        account,
        worker.id,
        &deployment_a,
        generation_a,
        "/ids",
    )
    .await;
    assert_eq!(ids.status, 200, "{}", ids.body);
    let identity: serde_json::Value = serde_json::from_str(&ids.body).unwrap();
    let named_id = identity["named"].as_str().unwrap();
    assert_eq!(named_id.len(), 64);
    assert_eq!(identity["named"], identity["namedAgain"]);
    assert_ne!(identity["named"], identity["unique"]);
    assert_eq!(identity["crossNamespaceRejected"], true);
    assert_eq!(identity["uppercaseRejected"], true);
    assert_eq!(identity["placementRejected"], true);
    assert_eq!(identity["mutatedIntrinsicNamed"], identity["named"]);
    assert!(
        DurableObjectId::from_str(named_id)
            .unwrap()
            .belongs_to(counter)
    );
    let (prefix, name_key) = DurableObjectRepository::new(&storage)
        .facade_identity(counter)
        .unwrap();
    let mut expected = Vec::from(prefix);
    let mut mac = <Hmac<Sha256>>::new_from_slice(&name_key).unwrap();
    mac.update(b"alpha");
    expected.extend_from_slice(&mac.finalize().into_bytes()[..24]);
    assert_eq!(named_id, hex::encode(expected));

    let first = dispatch(
        &transport,
        account,
        worker.id,
        &deployment_a,
        generation_a,
        "/increment?name=alpha",
    )
    .await;
    if first.status != 200 {
        let failed_pid = supervisor.snapshot().pid.unwrap();
        supervisor.report_unhealthy();
        wait_pid_change(&supervisor, failed_pid, Duration::from_secs(30)).await;
        panic!(
            "first DO dispatch failed: {}; diagnostics={:?}",
            first.body,
            supervisor.last_diagnostics()
        );
    }
    assert_eq!(first.body, "A:1");
    let second = dispatch(
        &transport,
        account,
        worker.id,
        &deployment_a,
        generation_a,
        "/rpc?name=alpha",
    )
    .await;
    if second.status != 200 {
        let failed_pid = supervisor.snapshot().pid.unwrap();
        supervisor.report_unhealthy();
        wait_pid_change(&supervisor, failed_pid, Duration::from_secs(30)).await;
        panic!(
            "DO RPC failed: {}; diagnostics={:?}",
            second.body,
            supervisor.last_diagnostics()
        );
    }
    assert_eq!(second.body, "A:1");
    let binary_rpc = dispatch(
        &transport,
        account,
        worker.id,
        &deployment_a,
        generation_a,
        "/rpc-binary?name=alpha",
    )
    .await;
    assert_eq!(
        (binary_rpc.status, binary_rpc.body.as_str()),
        (200, "4,5,6")
    );
    let rollback = dispatch(
        &transport,
        account,
        worker.id,
        &deployment_a,
        generation_a,
        "/rollback?name=alpha",
    )
    .await;
    assert_eq!((rollback.status, rollback.body.as_str()), (200, "true:1"));
    let websocket = dispatch(
        &transport,
        account,
        worker.id,
        &deployment_a,
        generation_a,
        "/websocket?name=alpha",
    )
    .await;
    if websocket.status != 200 {
        let failed_pid = supervisor.snapshot().pid.unwrap();
        supervisor.report_unhealthy();
        wait_pid_change(&supervisor, failed_pid, Duration::from_secs(30)).await;
        panic!(
            "DO websocket failed: {}; diagnostics={:?}",
            websocket.body,
            supervisor.last_diagnostics()
        );
    }
    assert_eq!(websocket.body, "text:true,binary:true");

    let storage_matrix = dispatch(
        &transport,
        account,
        worker.id,
        &deployment_a,
        generation_a,
        "/storage?name=storage-matrix",
    )
    .await;
    assert_eq!(storage_matrix.status, 200, "{}", storage_matrix.body);
    let storage_result: serde_json::Value = serde_json::from_str(&storage_matrix.body).unwrap();
    for key in [
        "syncKv",
        "asyncKv",
        "asyncTransactionRollback",
        "deleteAll",
        "blockConcurrency",
        "waitUntil",
    ] {
        assert_eq!(storage_result[key], true, "{key}: {storage_result}");
    }

    let ordered = dispatch(
        &transport,
        account,
        worker.id,
        &deployment_a,
        generation_a,
        "/order?name=ordered",
    )
    .await;
    assert_eq!(ordered.status, 200, "{}", ordered.body);
    let order: Vec<String> = serde_json::from_str(&ordered.body).unwrap();
    let first_start = order.iter().position(|item| item == "first:start").unwrap();
    let second_start = order
        .iter()
        .position(|item| item == "second:start")
        .unwrap();
    assert!(first_start < second_start, "same-stub E-order: {order:?}");

    let parallel_start = Instant::now();
    let (left, right) = tokio::join!(
        dispatch(
            &transport,
            account,
            worker.id,
            &deployment_a,
            generation_a,
            "/hold?name=left&ms=250",
        ),
        dispatch(
            &transport,
            account,
            worker.id,
            &deployment_a,
            generation_a,
            "/hold?name=right&ms=250",
        ),
    );
    assert_eq!((left.status, right.status), (200, 200));
    assert!(parallel_start.elapsed() < Duration::from_millis(450));

    let mut missing_class = deployment_request(
        account,
        worker.id,
        counter,
        other,
        "missing-class",
        "invalid",
        29,
        false,
    );
    missing_class.bundle = CanonicalBundle::build(
        "index.js",
        vec![ModuleInput {
            name: "index.js".to_owned(),
            module_type: ModuleType::EsModule,
            bytes: b"export default { fetch() { return new Response('missing'); } };".to_vec(),
        }],
        BundleLimits::default(),
    )
    .unwrap()
    .into_bytes()
    .into();
    assert_eq!(
        deployments
            .create_deployment(missing_class)
            .await
            .unwrap_err()
            .code(),
        open_compute_core::ErrorCode::DoClassNotFound
    );

    let in_flight = tokio::spawn({
        let transport = transport.clone();
        let deployment = deployment_a.clone();
        async move {
            dispatch(
                &transport,
                account,
                worker.id,
                &deployment,
                generation_a,
                "/hold?name=alpha&ms=3000",
            )
            .await
        }
    });
    tokio::time::sleep(Duration::from_millis(100)).await;

    let deployment_b = deploy(
        &deployments,
        deployment_request(
            account, worker.id, counter, other, "deploy-b", "B", 30, true,
        ),
        &supervisor,
    )
    .await;
    let completed_in_flight = in_flight.await.unwrap();
    assert_eq!(
        (
            completed_in_flight.status,
            completed_in_flight.body.as_str()
        ),
        (200, "A:2")
    );
    let generation_b = workers
        .get_worker(account, worker.id)
        .unwrap()
        .route_generation;
    assert!(generation_b > generation_a);
    let promoted = dispatch(
        &transport,
        account,
        worker.id,
        &deployment_b,
        generation_b,
        "/increment?name=alpha",
    )
    .await;
    assert_eq!((promoted.status, promoted.body.as_str()), (200, "B:3"));
    let stale = dispatch(
        &transport,
        account,
        worker.id,
        &deployment_a,
        generation_a,
        "/increment?name=alpha",
    )
    .await;
    assert_eq!(stale.status, 500);

    workers
        .promote_checked(
            account,
            worker.id,
            deployment_a.id,
            Some(deployment_b.id),
            Some(generation_b),
            RequestId::generate(),
            40,
        )
        .unwrap();
    let generation_rollback = workers
        .get_worker(account, worker.id)
        .unwrap()
        .route_generation;
    let rolled = dispatch(
        &transport,
        account,
        worker.id,
        &deployment_a,
        generation_rollback,
        "/rpc?name=alpha",
    )
    .await;
    assert_eq!((rolled.status, rolled.body.as_str()), (200, "A:3"));

    let old_pid = supervisor.snapshot().pid.unwrap();
    supervisor.report_unhealthy();
    wait_pid_change(&supervisor, old_pid, Duration::from_secs(30)).await;
    let recovered = dispatch(
        &transport,
        account,
        worker.id,
        &deployment_a,
        generation_rollback,
        "/rpc?name=alpha",
    )
    .await;
    assert_eq!((recovered.status, recovered.body.as_str()), (200, "A:3"));

    let object_id = DurableObjectId::from_str(named_id).unwrap();
    let repository = DurableObjectRepository::new(&storage);
    let fenced = repository
        .begin_object_delete(account, counter, object_id, 50)
        .unwrap();
    let authority = repository
        .deletion_authority(account, counter, object_id, fenced.generation)
        .unwrap();
    transport.delete_durable_object(&authority).await.unwrap();
    repository
        .finish_object_delete(counter, object_id, fenced.generation, 51)
        .unwrap();
    let recreated = dispatch(
        &transport,
        account,
        worker.id,
        &deployment_a,
        generation_rollback,
        "/increment?name=alpha",
    )
    .await;
    assert_eq!((recreated.status, recreated.body.as_str()), (200, "A:1"));
    let alpha_generations = repository
        .list_objects(account, counter)
        .unwrap()
        .into_iter()
        .filter(|object| object.object_id == object_id)
        .map(|object| object.generation)
        .collect::<Vec<_>>();
    assert_eq!(alpha_generations, vec![1, 2]);

    workers
        .delete_worker(account, worker.id, RequestId::generate(), 60)
        .unwrap();
    let fenced_after_worker_delete = repository
        .begin_object_delete(account, counter, object_id, 61)
        .unwrap();
    let purge_authority = repository
        .deletion_authority(
            account,
            counter,
            object_id,
            fenced_after_worker_delete.generation,
        )
        .unwrap();
    transport
        .delete_durable_object(&purge_authority)
        .await
        .unwrap();
    repository
        .finish_object_delete(
            counter,
            object_id,
            fenced_after_worker_delete.generation,
            62,
        )
        .unwrap();

    supervisor.shutdown().await;
    assert_eq!(supervisor.owner_registry_len(), 0);
    let _ = shutdown_tx.send(true);
    source_task.await.unwrap().unwrap();
    binding_task.await.unwrap().unwrap();
    println!("P0.7 identity/fetch/RPC/SQL/parallel/promotion/rollback/restart/delete/purge PASS");
}

fn create_namespace(
    storage: &PlatformStorage,
    pins: ResourcePins,
    account_id: AccountId,
    worker_id: WorkerId,
    class_name: &str,
    key: &str,
    now_ms: i64,
) -> ResourceId {
    let driver = DurableObjectResourceDriver::new(storage, worker_id, class_name);
    match ResourceController::new(storage, pins, driver)
        .create(&CreateResourceRequest {
            account_id,
            kind: BindingKind::DoNamespace,
            name: format!("{key}-namespace"),
            idempotency_key: format!("p0-7-{key}"),
            driver_schema_version: DO_NAMESPACE_SCHEMA_VERSION,
            request_id: RequestId::generate(),
            now_ms,
        })
        .unwrap()
    {
        CreateResourceOutcome::Applied(value) => value.resource_id,
        CreateResourceOutcome::Replay(_) => panic!("unexpected namespace replay"),
    }
}

async fn deploy(
    controller: &DeploymentController<'_>,
    request: CreateDeploymentRequest,
    supervisor: &WorkerdSupervisor,
) -> DeploymentRecord {
    match controller
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
    }
}

#[allow(clippy::too_many_arguments)]
fn deployment_request(
    account_id: AccountId,
    worker_id: WorkerId,
    counter: ResourceId,
    other: ResourceId,
    key: &str,
    release: &str,
    now_ms: i64,
    promote: bool,
) -> CreateDeploymentRequest {
    let bundle = CanonicalBundle::build(
        "index.js",
        vec![ModuleInput {
            name: "index.js".to_owned(),
            module_type: ModuleType::EsModule,
            bytes: do_source().as_bytes().to_vec(),
        }],
        BundleLimits::default(),
    )
    .unwrap();
    let mut bindings = BTreeMap::new();
    for (name, id) in [("COUNTER", counter), ("OTHER", other)] {
        bindings.insert(
            name.to_owned(),
            DeploymentBindingInput {
                capability_version: 1,
                kind: BindingKind::DoNamespace,
                id,
                permissions: CanonicalPermissions::default(),
                config: CanonicalBindingConfig::default(),
            },
        );
    }
    let mut vars = BTreeMap::new();
    vars.insert("RELEASE".to_owned(), serde_json::json!(release));
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
        promote,
        request_id: RequestId::generate(),
        now_ms,
    }
}

fn do_source() -> &'static str {
    r#"import { DurableObject } from "cloudflare:workers";

function read(sql) {
  const rows = sql.exec("SELECT value FROM counter WHERE id = 1").toArray();
  return rows.length ? Number(rows[0].value) : 0;
}
function increment(sql) {
  sql.exec("INSERT INTO counter(id, value) VALUES(1, 1) ON CONFLICT(id) DO UPDATE SET value = value + 1");
  return read(sql);
}

export class Counter extends DurableObject {
  constructor(ctx, env) {
    super(ctx, env);
    this.ctx.storage.sql.exec("CREATE TABLE IF NOT EXISTS counter(id INTEGER PRIMARY KEY, value INTEGER NOT NULL)");
  }
  async fetch(request) {
    const url = new URL(request.url);
    for (const name of [
      "x-open-compute-binding-token",
      "x-open-compute-account-id",
      "x-open-compute-worker-id",
      "x-open-compute-binding-id",
      "x-open-compute-deployment-id",
      "x-open-compute-descriptor-sha256",
      "x-open-compute-worker-code-sha256",
      "x-open-compute-route-generation",
      "x-open-compute-namespace-resource-id",
      "x-open-compute-object-id",
      "x-open-compute-object-generation",
      "x-open-compute-class-name",
      "x-open-compute-do-operation",
      "x-open-compute-request-id",
      "x-open-compute-startup-generation",
    ]) {
      if (request.headers.has(name)) return new Response("internal header leaked", { status: 500 });
    }
    if (url.searchParams.get("websocket") === "1") {
      const pair = new WebSocketPair();
      const [client, server] = Object.values(pair);
      server.accept();
      server.addEventListener("message", async event => {
        const value = event.data instanceof Blob ? await event.data.arrayBuffer() : event.data;
        server.send(value);
      });
      return new Response(null, { status: 101, webSocket: client });
    }
    const hold = Number(url.searchParams.get("hold") || 0);
    if (hold > 0) await scheduler.wait(hold);
    const value = this.ctx.storage.transactionSync(() => increment(this.ctx.storage.sql));
    await this.ctx.storage.sync();
    return new Response(`${this.env.RELEASE}:${value}`);
  }
  async getValue() { return { release: this.env.RELEASE, value: read(this.ctx.storage.sql) }; }
  async echoBinary(value) { return value; }
  async rollback() {
    const before = read(this.ctx.storage.sql);
    try {
      this.ctx.storage.transactionSync(() => { increment(this.ctx.storage.sql); throw new Error("rollback"); });
    } catch {}
    return { rolledBack: read(this.ctx.storage.sql) === before, value: read(this.ctx.storage.sql) };
  }
  async storageMatrix() {
    const result = {};
    try {
      this.ctx.storage.kv.put("sync", { value: 1 });
      const value = this.ctx.storage.kv.get("sync");
      const listed = [...this.ctx.storage.kv.list()].some(([key]) => key === "sync");
      result.syncKv = value.value === 1 && listed && this.ctx.storage.kv.delete("sync");
    } catch { return { failedStage: "syncKv" }; }
    try {
      await this.ctx.storage.put("async", { value: 2 });
      const value = await this.ctx.storage.get("async");
      const listed = (await this.ctx.storage.list()).has("async");
      result.asyncKv = value.value === 2 && listed && await this.ctx.storage.delete("async");
    } catch { return { failedStage: "asyncKv" }; }
    try {
      await this.ctx.storage.transaction(async txn => {
        await txn.put("rolled-back", 1);
        throw new Error("rollback");
      });
    } catch {
      result.asyncTransactionRollback = await this.ctx.storage.get("rolled-back") === undefined;
    }
    try {
      result.blockConcurrency = await this.ctx.blockConcurrencyWhile(async () => true);
    } catch { return { failedStage: "blockConcurrency" }; }
    try {
      const waited = Promise.resolve(true);
      this.ctx.waitUntil(waited);
      result.waitUntil = await waited;
    } catch { return { failedStage: "waitUntil" }; }
    try {
      await this.ctx.storage.put("delete-all", true);
      await this.ctx.storage.deleteAll();
      result.deleteAll = this.ctx.storage.kv.get("delete-all") === undefined
        && await this.ctx.storage.get("delete-all") === undefined;
      this.ctx.storage.sql.exec("CREATE TABLE IF NOT EXISTS counter(id INTEGER PRIMARY KEY, value INTEGER NOT NULL)");
    } catch (error) { return { failedStage: "deleteAll", detail: String(error) }; }
    return result;
  }
  async ordered(label, hold) {
    const order = this.ctx.storage.kv.get("order") || [];
    order.push(`${label}:start`);
    this.ctx.storage.kv.put("order", order);
    if (hold > 0) await scheduler.wait(hold);
    const current = this.ctx.storage.kv.get("order") || [];
    current.push(`${label}:end`);
    this.ctx.storage.kv.put("order", current);
    return current;
  }
  async orderValue() { return this.ctx.storage.kv.get("order") || []; }
}

export class OtherCounter extends Counter {}

export default {
  async fetch(request, env) {
    const url = new URL(request.url);
    const name = url.searchParams.get("name") || "alpha";
    if (url.pathname === "/ids") {
      const named = env.COUNTER.idFromName("alpha");
      const originalTextEncoder = globalThis.TextEncoder;
      globalThis.TextEncoder = class { encode() { throw new Error("mutated"); } };
      const mutatedIntrinsicNamed = env.COUNTER.idFromName("alpha").toString();
      globalThis.TextEncoder = originalTextEncoder;
      let crossNamespaceRejected = false;
      try { env.OTHER.idFromString(named.toString()); } catch { crossNamespaceRejected = true; }
      let uppercaseRejected = false;
      try { env.COUNTER.idFromString(named.toString().toUpperCase()); } catch { uppercaseRejected = true; }
      let placementRejected = false;
      try { env.COUNTER.getByName("alpha", { jurisdiction: "eu" }); } catch { placementRejected = true; }
      return Response.json({
        named: named.toString(),
        namedAgain: env.COUNTER.idFromName("alpha").toString(),
        unique: env.COUNTER.newUniqueId().toString(),
        mutatedIntrinsicNamed,
        crossNamespaceRejected,
        uppercaseRejected,
        placementRejected,
      });
    }
    const stub = env.COUNTER.getByName(name);
    if (url.pathname === "/rpc") {
      const result = await stub.getValue();
      return new Response(`${result.release}:${result.value}`);
    }
    if (url.pathname === "/rpc-binary") {
      const result = await stub.echoBinary(new Uint8Array([4, 5, 6]));
      return new Response(Array.from(new Uint8Array(result)).join(","));
    }
    if (url.pathname === "/rollback") {
      const result = await stub.rollback();
      return new Response(`${result.rolledBack}:${result.value}`);
    }
    if (url.pathname === "/storage") {
      return Response.json(await stub.storageMatrix());
    }
    if (url.pathname === "/order") {
      await Promise.all([
        stub.ordered("first", 80),
        stub.ordered("second", 0),
      ]);
      return Response.json(await stub.orderValue());
    }
    if (url.pathname === "/websocket") {
      const response = await stub.fetch(new Request("https://object.invalid/?websocket=1", {
        headers: { Connection: "Upgrade", Upgrade: "websocket" },
      }));
      const socket = response.webSocket;
      if (!socket) return new Response("missing websocket", { status: 500 });
      socket.accept();
      const next = () => Promise.race([
        new Promise(resolve => socket.addEventListener("message", event => resolve(event.data), { once: true })),
        scheduler.wait(2000).then(() => { throw new Error("websocket timeout"); }),
      ]);
      socket.send("ping");
      const text = await next();
      socket.send(new Uint8Array([1, 2, 3]));
      const binary = await next();
      socket.close(1000, "done");
      let binaryBytes = binary instanceof ArrayBuffer
        ? new Uint8Array(binary)
        : ArrayBuffer.isView(binary) ? new Uint8Array(binary.buffer, binary.byteOffset, binary.byteLength) : null;
      if (!binaryBytes && binary instanceof Blob) binaryBytes = new Uint8Array(await binary.arrayBuffer());
      const binaryOk = Boolean(binaryBytes) && Array.from(binaryBytes).join(",") === "1,2,3";
      return new Response(`text:${text === "ping"},binary:${binaryOk}`);
    }
    const hold = url.pathname === "/hold" ? url.searchParams.get("ms") || "0" : "0";
    return stub.fetch(`https://object.invalid/?hold=${hold}`);
  }
};
"#
}

struct DispatchResponse {
    status: u16,
    body: String,
}

async fn dispatch(
    transport: &WorkerdTransport,
    account_id: AccountId,
    worker_id: WorkerId,
    deployment: &DeploymentRecord,
    route_generation: u64,
    path: &str,
) -> DispatchResponse {
    let request = Request::builder()
        .method("POST")
        .uri(path)
        .header(header::HOST, "do.test")
        .body(Body::empty())
        .unwrap();
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
            request,
        )
        .await
        .unwrap();
    let status = response.status().as_u16();
    let bytes = to_bytes(response.into_body(), 8 * 1024 * 1024)
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
            snapshot.state != SupervisorState::Failed && start.elapsed() < timeout,
            "runtime failed: {snapshot:?}; diagnostics={:?}",
            supervisor.last_diagnostics()
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
    ArtifactStore::new(S3ArtifactClient::connect(&config, &credentials, 64 * 1024 * 1024).unwrap())
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
