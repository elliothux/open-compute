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
    serve_binding_backend_with_products,
};
use open_compute_storage::{
    D1_DATABASE_SCHEMA_VERSION, DeploymentRecord, PlatformStorage, R2_SCHEMA_VERSION,
    ReserveResourceCreate, ResourceCreateReservation, ResourceRepository, WorkerRepository,
};
use open_compute_workers::{
    BundleLimits, CanonicalBundle, CreateDeploymentOutcome, CreateDeploymentRequest,
    D1ResourceDriver, DeploymentBindingInput, DeploymentController, ModuleInput, ModuleType,
    R2ResourceDriver, ResourceDriver, ResourcePins, RuntimeSource, RuntimeValidator,
};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn p0_6_real_d1_facade_and_backend_matrix() {
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
            serve_binding_backend_with_products(
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
    let supervisor = Arc::new(WorkerdSupervisor::new_with_services_and_auth(
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
    let database = create_database(&storage, &d1_config, account);
    let bucket = create_bucket(&storage, &objects, &r2_config, account).await;
    let repository = WorkerRepository::new(storage.db());
    let validator: Arc<dyn RuntimeValidator> = Arc::new(transport.clone());
    let deployments =
        DeploymentController::new(&storage, artifacts, validator, BundleLimits::default());
    let (worker, _) = repository
        .create_worker(account, "d1-matrix", RequestId::generate(), 20)
        .unwrap();
    let deployment = deploy(
        &deployments,
        deployment_request(
            account,
            worker.id,
            database,
            Some(bucket),
            "matrix-v1",
            matrix_source(),
            21,
        ),
        &supervisor,
    )
    .await;
    let cold = dispatch(&transport, account, worker.id, &deployment, None, "/matrix").await;
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

    d1_service.arm_response_loss_once();
    let batch_loss = dispatch(
        &transport,
        account,
        worker.id,
        &deployment,
        None,
        "/batch-loss",
    )
    .await;
    assert_eq!((batch_loss.status, batch_loss.body.as_str()), (200, "true"));

    let warm = dispatch(&transport, account, worker.id, &deployment, None, "/count").await;
    assert_eq!((warm.status, warm.body.as_str()), (200, "2"));
    assert_eq!(warm.loader_outcome, Some(LoaderOutcome::Warm));
    let named = dispatch(
        &transport,
        account,
        worker.id,
        &deployment,
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
            .create_worker(account, name, RequestId::generate(), now)
            .unwrap();
        let shape = deploy(
            &deployments,
            deployment_request(
                account,
                shape_worker.id,
                database,
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
    let restarted = dispatch(&transport, account, worker.id, &deployment, None, "/count").await;
    assert_eq!((restarted.status, restarted.body.as_str()), (200, "2"));
    assert_eq!(pins.count(database), 0);
    supervisor.shutdown().await;
    assert_eq!(supervisor.owner_registry_len(), 0);
    let _ = shutdown_tx.send(true);
    source_task.await.unwrap().unwrap();
    binding_task.await.unwrap().unwrap();
    println!("P0.6 DF-01..DF-12 facade/SQLite/restart matrix PASS");
}

fn create_database(storage: &PlatformStorage, config: &D1Config, account: AccountId) -> ResourceId {
    let resource = reserve(
        storage,
        account,
        BindingKind::D1Database,
        D1_DATABASE_SCHEMA_VERSION,
        "d1",
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
        .reserve_create(&ReserveResourceCreate {
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
        })
        .unwrap();
    let ResourceCreateReservation::Reserved(resource) = reservation else {
        panic!("unexpected reservation")
    };
    resource
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
    worker_id: open_compute_core::WorkerId,
    database: ResourceId,
    bucket: Option<ResourceId>,
    key: &str,
    source: &str,
    now_ms: i64,
) -> CreateDeploymentRequest {
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
            DeploymentBindingInput {
                capability_version: 1,
                kind: BindingKind::D1Database,
                id: database,
                permissions: CanonicalPermissions::default(),
                config: CanonicalBindingConfig::default(),
            },
        );
    }
    if let Some(bucket) = bucket {
        bindings.insert(
            "BUCKET".to_owned(),
            DeploymentBindingInput {
                capability_version: 1,
                kind: BindingKind::R2Bucket,
                id: bucket,
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
        vars: BTreeMap::new(),
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
import { D1Database as ImportableD1Database } from "./__open_compute__/d1/facade.js";
import { R2Bucket as ImportableR2Bucket } from "./__open_compute__/r2/facade.js";

const meta = () => ({
  served_by: "open-compute-local", served_by_primary: true, duration: 0,
  changes: 0, last_row_id: 0, changed_db: false, size_after: 0,
  rows_read: 1, rows_written: 0,
});
const fakeResult = (columns = ["value"], rows = [[1]]) => ({ results: [{ columns, rows, meta: meta() }] });
const codeOf = (error) => String(error && error.message || error);
const syncThrows = (fn, code) => {
  try { fn(); return false; } catch (error) { return codeOf(error).includes(code); }
};
const rejects = async (fn, code) => {
  try { await fn(); return false; } catch (error) { return codeOf(error).includes(code); }
};

export class Named extends WorkerEntrypoint {
  constructor(ctx, env) { super(ctx, env); this.wrapped = env.DB instanceof ImportableD1Database; }
  async fetch() { return new Response(`named:${this.wrapped}`); }
}

export default {
  async fetch(request, env) {
    const path = new URL(request.url).pathname;
    if (path === "/count") return new Response(String((await env.DB.prepare("SELECT count(*) AS n FROM items").first("n"))));
    if (path === "/batch-loss") {
      try {
        await env.DB.batch([
          env.DB.prepare("INSERT INTO items(value) VALUES ('lost-batch-a')"),
          env.DB.prepare("INSERT INTO items(value) VALUES ('lost-batch-b')"),
        ]);
        return new Response("false");
      } catch (error) {
        const committed = (await env.DB.prepare(
          "SELECT count(*) AS n FROM items WHERE value IN ('lost-batch-a', 'lost-batch-b')",
        ).first("n")) === 2;
        await env.DB.prepare(
          "DELETE FROM items WHERE value IN ('lost-batch-a', 'lost-batch-b')",
        ).run();
        return new Response(String(codeOf(error).includes("D1_RESULT_UNKNOWN") && committed));
      }
    }
    if (path !== "/matrix") return new Response("missing", { status: 404 });
    try {
      const calls = [];
      const raw = {
        async query(mode, statements) {
          calls.push({ mode, statements });
          if (mode === "batch") return { results: statements.map(() => fakeResult().results[0]) };
          if (statements[0].sql === "magic") return fakeResult(["__proto__", "constructor"], [["safe", "also-safe"]]);
          if (statements[0].sql === "empty") return fakeResult(["value"], []);
          return fakeResult();
        },
        async exec(sql) { calls.push({ exec: sql }); return { count: 1, duration: 0 }; },
      };
      const fake = new ImportableD1Database(raw);
      const prepared = fake.prepare("SELECT ?1");
      const df01 = prepared && calls.length === 0;
      const view = new Uint8Array([9, 1, 2, 9]).subarray(1, 3);
      const bound = prepared.bind(view);
      const reused = prepared.bind("again");
      const df02 = bound !== prepared && reused !== prepared && calls.length === 0;
      await bound.all();
      const df03 = calls.length === 1 && calls[0].mode === "all"
        && calls[0].statements.length === 1 && calls[0].statements[0].params[0] instanceof Uint8Array;
      const beforeBatch = calls.length;
      const batch = await fake.batch([bound, reused, bound]);
      const df04 = batch.length === 3 && calls.length === beforeBatch + 1
        && calls.at(-1).statements.length === 3;
      const other = new ImportableD1Database(raw);
      const session = fake.withSession("first-primary");
      const df05 = await rejects(() => fake.batch([other.prepare("SELECT 1")]), "D1_INVALID_BATCH")
        && await rejects(() => fake.batch([session.prepare("SELECT 1")]), "D1_INVALID_BATCH")
        && await rejects(() => fake.batch([{}]), "D1_INVALID_BATCH");
      const rejected = [undefined, 1n, NaN, Infinity, {}, new Date(), () => {}, Symbol("x")]
        .every((value) => syncThrows(() => prepared.bind(value), "D1_TYPE_ERROR"));
      const df06 = rejected && prepared.bind(null, true, false, 1, 1.5, "x", new ArrayBuffer(0));
      const df07 = calls[0].statements[0].params[0].byteLength === 2
        && calls[0].statements[0].params[0][0] === 1 && calls[0].statements[0].params[0][1] === 2;
      const magic = await fake.prepare("magic").first();
      const df08 = Object.getPrototypeOf(magic) === Object.prototype
        && Object.prototype.hasOwnProperty.call(magic, "__proto__") && magic.__proto__ === "safe";
      const df09 = env.DB instanceof ImportableD1Database && env.DB_ALIAS instanceof ImportableD1Database;
      const df10 = Object.keys(env).sort().join(",") === "BUCKET,DB,DB_ALIAS"
        && !Reflect.ownKeys(env.DB).some((key) => String(key).includes("raw"))
        && typeof env.DB.fetch === "undefined";
      const df11 = df09;
      const df12 = env.BUCKET instanceof ImportableR2Bucket
        && typeof env.BUCKET.head === "function" && typeof env.BUCKET.fetch === "undefined"
        && await env.BUCKET.head("missing") === null;

      let resultUnknown = false;
      try {
        await env.DB.exec("CREATE TABLE items(id INTEGER PRIMARY KEY, value TEXT UNIQUE, data BLOB)");
      } catch (error) {
        resultUnknown = codeOf(error).includes("D1_RESULT_UNKNOWN")
          && (await env.DB.prepare("SELECT count(*) AS n FROM sqlite_master WHERE type='table' AND name='items'").first("n")) === 1;
      }
      await env.DB.prepare("INSERT INTO items(value, data) VALUES (?1, ?2)").bind("one", view).run();
      await env.DB.batch([
        env.DB.prepare("INSERT INTO items(value) VALUES (?1)").bind("two"),
        env.DB.prepare("SELECT count(*) FROM items"),
      ]);
      const real = await env.DB.prepare("SELECT id, value FROM items ORDER BY id").raw();
      const blob = await env.DB.prepare("SELECT data FROM items WHERE id = 1").first("data");
      let batchRollback = false;
      try {
        await env.DB.batch([
          env.DB.prepare("INSERT INTO items(value) VALUES ('three')"),
          env.DB.prepare("INSERT INTO items(value) VALUES ('one')"),
        ]);
      } catch {
        batchRollback = (await env.DB.prepare("SELECT count(*) AS n FROM items WHERE value='three'").first("n")) === 0;
      }
      let execPrefix = false;
      try {
        await env.DB.exec("INSERT INTO items(value) VALUES ('prefix'); SELECT * FROM absent; INSERT INTO items(value) VALUES ('never')");
      } catch {
        execPrefix = (await env.DB.prepare("SELECT count(*) AS n FROM items WHERE value='prefix'").first("n")) === 1;
        await env.DB.prepare("DELETE FROM items WHERE value='prefix'").run();
      }
      const denied = [];
      for (const sql of ["ATTACH DATABASE ':memory:' AS other", "PRAGMA writable_schema=ON", "BEGIN", "SELECT * FROM __open_compute_meta"]) {
        try { await env.DB.exec(sql); denied.push(false); } catch (error) { denied.push(codeOf(error).includes("D1_AUTHORIZER_DENIED")); }
      }
      const firstNull = await fake.prepare("empty").first() === null;
      const rawColumns = JSON.stringify(await fake.prepare("magic").raw({ columnNames: true }))
        === JSON.stringify([["__proto__", "constructor"], ["safe", "also-safe"]]);
      const sqlFastLimit = syncThrows(() => fake.prepare("x".repeat(100001)), "D1_SQL_INVALID");
      const batchFastLimit = await rejects(
        () => fake.batch(Array.from({ length: 101 }, () => prepared)), "D1_INVALID_BATCH",
      );
      const parameterLimit = await rejects(
        () => env.DB.prepare("SELECT 1").bind(...Array(101).fill(null)).all(), "D1_LIMIT_ERROR",
      );
      const rowLimit = await rejects(
        () => env.DB.prepare("SELECT 1 UNION ALL SELECT 2 UNION ALL SELECT 3").all(),
        "D1_LIMIT_ERROR",
      );
      const resultLimit = await rejects(
        () => env.DB.prepare("SELECT printf('%2000s', 'x')").all(), "D1_LIMIT_ERROR",
      );
      const vmLimit = await rejects(
        () => env.DB.prepare("WITH RECURSIVE c(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM c WHERE x<100000) SELECT sum(x) FROM c").all(),
        "D1_LIMIT_ERROR",
      );
      const limitMatrix = sqlFastLimit && batchFastLimit && parameterLimit
        && rowLimit && resultLimit && vmLimit;
      return Response.json({
        df01, df02, df03, df04, df05, df06: Boolean(df06), df07, df08, df09, df10, df11, df12,
        realRows: real, blob, batchRollback, execPrefix, authorizer: denied.every(Boolean),
        resultUnknown, firstNull, rawColumns, limitMatrix,
      });
    } catch (error) {
      return new Response(error && error.stack ? error.stack : String(error), { status: 598 });
    }
  }
};"#
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
    deployment: &DeploymentRecord,
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
                deployment_id: deployment.id,
                worker_code_sha256: hex::encode(deployment.worker_code_sha256),
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
