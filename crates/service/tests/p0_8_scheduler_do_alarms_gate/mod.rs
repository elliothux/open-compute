//! Real pinned-workerd P0.8 scheduler and Durable Object alarm conformance Gate.

#![cfg(feature = "test-support")]

use axum::body::{Body, to_bytes};
use axum::http::{Request, header};
use open_compute_artifacts::{
    ArtifactStore, MapEnv, MockS3, ObjectBackend, resolve_s3_credentials_with,
};
use open_compute_core::clock::SystemClock;
use open_compute_core::config::{DataConfig, DurableObjectsConfig, PlatformConfig, RuntimeConfig};
use open_compute_core::{
    AccountId, BindingKind, CanonicalBindingConfig, CanonicalPermissions, MetricsConfig, RequestId,
    ResourceId, SchedulerConfig, SystemSchedulerClock, WorkerId,
};
use open_compute_runtime::{
    DirectoryServicePath, ExternalServiceAddress, GenerationAuthRegistry, OsJitter,
    PlatformReleaseMeta, StaticConfigCompiler, SupervisorState, WorkerdSupervisor,
    WorkerdSupervisorOptions, verify_runtime_binary,
};
use open_compute_service::runtime_bridge::{
    AlarmDispatchOutcome, DispatchTarget, WorkerdTransport, bind_runtime_source,
    serve_runtime_source,
};
use open_compute_service::scheduler::SchedulerService;
use open_compute_service::{
    HealthCoordinator, MetricsRegistry, SqliteKvBindingExecutor, bind_binding_backend,
    serve_binding_backend,
};
use open_compute_storage::{
    AlarmProjection, ClaimResult, DO_NAMESPACE_SCHEMA_VERSION, DurableObjectRepository,
    PlatformStorage, SchedulerStore, SchedulerSummary, VersionRecord, WorkerRepository,
};
use open_compute_workers::{
    BundleLimits, CanonicalBundle, CreateResourceOutcome, CreateResourceRequest,
    CreateVersionOutcome, CreateVersionRequest, DurableObjectResourceDriver, ModuleInput,
    ModuleType, ResourceController, ResourcePins, RuntimeSource, RuntimeValidator,
    VersionBindingInput, VersionController,
};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

mod p0_8_real_scheduler_alarm_matrix;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn p0_8_real_scheduler_alarm_matrix() {
    p0_8_real_scheduler_alarm_matrix::run().await;
}

fn durable_objects_config() -> DurableObjectsConfig {
    DurableObjectsConfig {
        disk_high_watermark_percent: 98,
        disk_stop_writes_percent: 99,
        ..DurableObjectsConfig::default()
    }
}

fn create_namespace(
    storage: &PlatformStorage,
    pins: ResourcePins,
    account_id: AccountId,
    worker_id: WorkerId,
    class_name: &str,
    now_ms: i64,
) -> ResourceId {
    let driver = DurableObjectResourceDriver::new(storage, worker_id, class_name);
    match ResourceController::new(storage, pins, driver)
        .create(&CreateResourceRequest {
            account_id,
            kind: BindingKind::DoNamespace,
            name: "alarm-namespace".to_owned(),
            idempotency_key: "p0-8-alarm".to_owned(),
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

fn version_request(
    account_id: AccountId,
    worker_id: WorkerId,
    namespace: ResourceId,
    key: &str,
    release: &str,
    now_ms: i64,
    promote: bool,
) -> CreateVersionRequest {
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
    bindings.insert(
        "ALARM".to_owned(),
        VersionBindingInput {
            kind: BindingKind::DoNamespace,
            id: namespace,
            permissions: CanonicalPermissions::default(),
            config: CanonicalBindingConfig::default(),
        },
    );
    let mut vars = BTreeMap::new();
    vars.insert("RELEASE".to_owned(), serde_json::json!(release));
    if let Some(config) = raw_tcp_fixture_json() {
        vars.insert(
            "RAW_TCP_CONFIG_JSON".to_owned(),
            serde_json::Value::String(config),
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
        vars,
        secrets: BTreeMap::new(),
        bindings,
        services: BTreeMap::new(),
        runtime_features: Default::default(),
        queue_consumers: Vec::new(),
        crons: Vec::new(),
        deployment_source: promote.then_some(open_compute_storage::DeploymentSource::ScriptUpload),
        request_id: RequestId::generate(),
        now_ms,
    }
}

fn do_source() -> &'static str {
    r#"import { DurableObject } from "cloudflare:workers";
import { connect } from "cloudflare:sockets";

async function rawTcpProbe(env) {
  if (!env.RAW_TCP_CONFIG_JSON) return false;
  const config = JSON.parse(env.RAW_TCP_CONFIG_JSON);
  const payload = new Uint8Array([17, 18, 19, 20]);
  const socket = connect({ hostname: config.hostname, port: Number(config.tcpPort) }, {
    allowHalfOpen: true, secureTransport: "off",
  });
  await socket.opened;
  const writer = socket.writable.getWriter();
  await writer.write(new TextEncoder().encode(`ECHO ${payload.byteLength}\n`));
  await writer.write(payload);
  await writer.close();
  writer.releaseLock();
  const echoed = new Uint8Array(await new Response(socket.readable).arrayBuffer());
  await socket.close();
  await socket.closed;
  let denied = false;
  const privateSocket = connect({
    hostname: config.privateHostname, port: Number(config.tcpPort),
  });
  try {
    await privateSocket.opened;
    await privateSocket.close();
  } catch {
    denied = true;
    try { await privateSocket.close(); } catch {}
  }
  if (echoed.length !== payload.length
      || !echoed.every((value, index) => value === payload[index]) || !denied) {
    throw new Error("DO raw TCP event-source policy mismatch");
  }
  return true;
}

function scalar(sql, query, fallback = 0) {
  const rows = sql.exec(query).toArray();
  return rows.length ? rows[0].value : fallback;
}

export class AlarmObject extends DurableObject {
  storageAtField = this.ctx.storage;
  constructor(ctx, env) {
    super(ctx, env);
    this.ctx = ctx;
    this.env = env;
    this.constructorStorage = ctx.storage;
    this.ctx.storage.sql.exec(
      "CREATE TABLE IF NOT EXISTS alarm_events(" +
      "id INTEGER PRIMARY KEY CHECK(id = 1), deliveries INTEGER NOT NULL, failures INTEGER NOT NULL, " +
      "last_release TEXT, last_retry_count INTEGER, last_is_retry INTEGER)"
    );
    this.ctx.storage.sql.exec(
      "INSERT INTO alarm_events(id, deliveries, failures) VALUES(1, 0, 0) ON CONFLICT(id) DO NOTHING"
    );
  }
  proxyStable() {
    return this.storageAtField === this.ctx.storage && this.ctx.storage === this.constructorStorage;
  }
  async setAt(time) { await this.ctx.storage.setAlarm(time); return this.ctx.storage.getAlarm(); }
  async setDate(time) { await this.ctx.storage.setAlarm(new Date(time)); return this.ctx.storage.getAlarm(); }
  async getAlarmValue() { return this.ctx.storage.getAlarm(); }
  async deleteAlarmValue() { await this.ctx.storage.deleteAlarm(); return true; }
  async invalidAlarm(kind) {
    const value = kind === "zero" ? 0 : kind === "nan" ? NaN : kind === "infinity" ? Infinity : "bad";
    try { await this.ctx.storage.setAlarm(value); return false; } catch { return true; }
  }
  async transactionCommit(time) {
    await this.ctx.storage.transaction(async txn => {
      await txn.put("transaction", "committed");
      await txn.setAlarm(time + 1);
      await txn.setAlarm(time);
    });
    return this.ctx.storage.getAlarm();
  }
  async transactionRollback(time) {
    try {
      await this.ctx.storage.transaction(async txn => {
        await txn.put("transaction", "rolled-back");
        await txn.setAlarm(time);
        throw new Error("rollback");
      });
    } catch {}
    const transactionValue = await this.ctx.storage.get("transaction");
    const alarmValue = await this.ctx.storage.getAlarm();
    return transactionValue === "committed" && alarmValue === null;
  }
  transactionSyncRejected() {
    try {
      this.ctx.storage.transactionSync(() => this.ctx.storage.setAlarm(Date.now() + 1000));
      return false;
    } catch (error) { return error instanceof TypeError; }
  }
  async failThenAlarm(count, time) {
    this.ctx.storage.sql.exec("UPDATE alarm_events SET failures = ? WHERE id = 1", count);
    await this.ctx.storage.setAlarm(time);
    return true;
  }
  async status() {
    const rows = this.ctx.storage.sql.exec(
      "SELECT deliveries, failures, last_release, last_retry_count, last_is_retry FROM alarm_events WHERE id = 1"
    ).toArray();
    const row = rows[0];
    return {
      alarm: await this.ctx.storage.getAlarm(),
      deliveries: Number(row.deliveries),
      failures: Number(row.failures),
      lastRelease: row.last_release,
      lastRetryCount: row.last_retry_count === null ? null : Number(row.last_retry_count),
      lastIsRetry: row.last_is_retry === null ? null : Number(row.last_is_retry) === 1,
      rawTcpAlarm: await this.ctx.storage.get("raw-tcp-alarm") === true,
    };
  }
  async deleteEverything(time) {
    await this.ctx.storage.put("delete-all-kv", true);
    this.ctx.storage.sql.exec("CREATE TABLE delete_all_sql(value INTEGER)");
    this.ctx.storage.sql.exec("INSERT INTO delete_all_sql VALUES(1)");
    this.ctx.storage.sql.exec("CREATE TABLE delete_all_parent(id INTEGER PRIMARY KEY)");
    this.ctx.storage.sql.exec(
      "CREATE TABLE delete_all_child(parent_id INTEGER REFERENCES delete_all_parent(id))"
    );
    this.ctx.storage.sql.exec("INSERT INTO delete_all_parent VALUES(1)");
    this.ctx.storage.sql.exec("INSERT INTO delete_all_child VALUES(1)");
    await this.ctx.storage.setAlarm(time);
    await this.ctx.storage.deleteAll();
    const kvGone = await this.ctx.storage.get("delete-all-kv") === undefined;
    const tables = this.ctx.storage.sql.exec(
      "SELECT name FROM sqlite_master WHERE name IN " +
      "('delete_all_sql', 'delete_all_parent', 'delete_all_child')"
    ).toArray();
    this.ctx.storage.sql.exec(
      "CREATE TABLE IF NOT EXISTS alarm_events(" +
      "id INTEGER PRIMARY KEY CHECK(id = 1), deliveries INTEGER NOT NULL, failures INTEGER NOT NULL, " +
      "last_release TEXT, last_retry_count INTEGER, last_is_retry INTEGER)"
    );
    this.ctx.storage.sql.exec(
      "INSERT INTO alarm_events(id, deliveries, failures) VALUES(1, 0, 0) ON CONFLICT(id) DO NOTHING"
    );
    return kvGone && tables.length === 0;
  }
  async alarm(info) {
    if (this.env.RAW_TCP_CONFIG_JSON
        && await this.ctx.storage.get("raw-tcp-alarm") !== true) {
      await rawTcpProbe(this.env);
      await this.ctx.storage.put("raw-tcp-alarm", true);
    }
    this.ctx.storage.sql.exec(
      "UPDATE alarm_events SET deliveries = deliveries + 1, last_release = ?, " +
      "last_retry_count = ?, last_is_retry = ? WHERE id = 1",
      this.env.RELEASE, info.retryCount, info.isRetry ? 1 : 0
    );
    const failures = Number(scalar(this.ctx.storage.sql, "SELECT failures AS value FROM alarm_events WHERE id = 1"));
    if (failures > 0) {
      this.ctx.storage.sql.exec("UPDATE alarm_events SET failures = failures - 1 WHERE id = 1");
      throw new Error("expected alarm failure");
    }
  }
  async fetch(request) {
    const path = new URL(request.url).pathname;
    if (path === "/raw-tcp") {
      return Response.json({ probed: await rawTcpProbe(this.env) });
    }
    if (path === "/proxy") {
      return new Response(String(this.proxyStable()));
    }
    return new Response(null, { status: 404 });
  }
}

export default {
  async fetch(request, env) {
    const url = new URL(request.url);
    const stub = env.ALARM.getByName("singleton");
    const time = Number(url.searchParams.get("time"));
    if (url.pathname === "/proxy-rpc") return new Response(String(await stub.proxyStable()));
    if (url.pathname === "/proxy-fetch") {
      return stub.fetch(new Request("https://object.invalid/proxy"));
    }
    if (url.pathname === "/raw-tcp") {
      return stub.fetch(new Request("https://object.invalid/raw-tcp"));
    }
    if (url.pathname === "/set") return new Response(String(await stub.setAt(time)));
    if (url.pathname === "/set-date") return new Response(String(await stub.setDate(time)));
    if (url.pathname === "/get") return new Response(String(await stub.getAlarmValue()));
    if (url.pathname === "/delete") return new Response(String(await stub.deleteAlarmValue()));
    if (url.pathname === "/txn-commit") return new Response(String(await stub.transactionCommit(time)));
    if (url.pathname === "/txn-rollback") return new Response(String(await stub.transactionRollback(time)));
    if (url.pathname === "/txn-sync") return new Response(String(await stub.transactionSyncRejected()));
    if (url.pathname === "/status") return Response.json(await stub.status());
    if (url.pathname === "/fail") {
      await stub.failThenAlarm(Number(url.searchParams.get("count")), time);
      return new Response("ok");
    }
    if (url.pathname === "/forge-private-alarm") {
      await stub.__openComputeAlarm({ rowToken: crypto.randomUUID(), retryCount: 0 });
      return new Response("forged");
    }
    if (url.pathname === "/delete-all") return new Response(String(await stub.deleteEverything(time)));
    if (url.pathname === "/invalid") {
      return Response.json({
        zero: await stub.invalidAlarm("zero"),
        nan: await stub.invalidAlarm("nan"),
        infinity: await stub.invalidAlarm("infinity"),
        type: await stub.invalidAlarm("type"),
      });
    }
    return new Response(null, { status: 404 });
  }
};
"#
}

struct DispatchResponse {
    status: u16,
    body: String,
}

async fn dispatch_path(
    transport: &WorkerdTransport,
    account_id: AccountId,
    worker_id: WorkerId,
    version: &VersionRecord,
    route_generation: u64,
    path: &str,
) -> DispatchResponse {
    dispatch(
        transport,
        account_id,
        worker_id,
        version,
        route_generation,
        path,
    )
    .await
}

async fn dispatch(
    transport: &WorkerdTransport,
    account_id: AccountId,
    worker_id: WorkerId,
    version: &VersionRecord,
    route_generation: u64,
    path: &str,
) -> DispatchResponse {
    let request = Request::builder()
        .method("POST")
        .uri(path)
        .header(header::HOST, "alarm.test")
        .body(Body::empty())
        .unwrap();
    let response = transport
        .dispatch(
            DispatchTarget {
                account_id,
                worker_id,
                version_id: version.id,
                worker_code_sha256: hex::encode(version.worker_code_sha256),
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

async fn status(
    transport: &WorkerdTransport,
    account_id: AccountId,
    worker_id: WorkerId,
    version: &VersionRecord,
    route_generation: u64,
) -> serde_json::Value {
    let response = dispatch(
        transport,
        account_id,
        worker_id,
        version,
        route_generation,
        "/status",
    )
    .await;
    assert_ok(&response);
    serde_json::from_str(&response.body).unwrap()
}

#[track_caller]
fn assert_ok(response: &DispatchResponse) {
    assert_eq!(response.status, 200, "{}", response.body);
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
[data]
path = "/var/lib/open-compute"
master_key_file = "/var/lib/open-compute/keys/master.key"

[storage]
backend = "s3"
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
    .object_storage
    .as_s3()
    .expect("S3 config")
    .clone();
    let env = MapEnv::new()
        .with("S3_ACCESS_KEY_ID", "AKIAEXAMPLEKEYID01")
        .with(
            "S3_SECRET_ACCESS_KEY",
            "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY",
        );
    let credentials = resolve_s3_credentials_with(&config, &env).unwrap();
    ArtifactStore::new(ObjectBackend::connect_s3(&config, &credentials, 64 * 1024 * 1024).unwrap())
}

fn raw_tcp_fixture_json() -> Option<String> {
    const NAMES: [&str; 3] = [
        "OPEN_COMPUTE_EGRESS_PUBLIC_HOSTNAME",
        "OPEN_COMPUTE_EGRESS_PRIVATE_HOSTNAME",
        "OPEN_COMPUTE_EGRESS_PUBLIC_TCP_PORT",
    ];
    let values = NAMES.map(std::env::var);
    if values.iter().all(Result::is_err) {
        return None;
    }
    let [hostname, private_hostname, tcp_port] =
        values.map(|value| value.expect("all raw TCP fixture values must be set"));
    Some(
        serde_json::json!({
            "hostname": hostname,
            "privateHostname": private_hostname,
            "tcpPort": tcp_port,
        })
        .to_string(),
    )
}

fn storage_config(root: &Path) -> DataConfig {
    DataConfig {
        path: root.to_path_buf(),
        master_key_file: root.join("keys/master.key"),
        master_key_env: None,
        sqlite_busy_timeout_ms: 5_000,
        free_space_soft_bytes: 1_073_741_824,
        free_space_hard_bytes: 1,
    }
}

fn runtime_config() -> RuntimeConfig {
    let mut config = PlatformConfig::local_test_config().runtime;
    config.startup_timeout_ms = 20_000;
    config.shutdown_grace_ms = 1_000;
    config.kill_timeout_ms = 2_000;
    config
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(i64::MAX)
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .unwrap()
        .to_path_buf()
}
