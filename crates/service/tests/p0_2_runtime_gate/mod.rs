//! Real pinned-workerd P0.2 dynamic Worker data-plane gate.

#![cfg(feature = "test-support")]

use axum::body::{Body, to_bytes};
use axum::http::{Request, header};
use base64::Engine as _;
use bytes::Bytes;
use futures::Stream;
use http_body_util::BodyExt as _;
use open_compute_artifacts::{
    ArtifactRef, ArtifactStore, MapEnv, MockS3, ObjectBackend, resolve_s3_credentials_with,
};
use open_compute_core::clock::SystemClock;
use open_compute_core::config::{DataConfig, PlatformConfig, RuntimeConfig, SecretReference};
use open_compute_core::{
    ComponentName, ComponentState, ErrorCode, MetricsConfig, QueueMessageId, ReadinessReason,
    Redactor, RequestId, SecretString, ServerConfig,
};
use open_compute_runtime::{
    DirectoryServicePath, ExternalServiceAddress, GenerationAuthRegistry, OsJitter,
    PlatformReleaseMeta, StaticConfigCompiler, SupervisorState, WorkerdSupervisor,
    WorkerdSupervisorOptions, verify_runtime_binary,
};
use open_compute_service::http::{HttpState, merged_router};
use open_compute_service::runtime_bridge::{
    DispatchTarget, LoaderOutcome, QueueDispatchMessage, QueueDispatchRequest,
    ScheduledDispatchRequest, WorkerdTransport, bind_runtime_source, serve_runtime_source,
};
use open_compute_service::workers_http::WorkerApiState;
use open_compute_service::workflow_http::WorkflowApiState;
use open_compute_service::{
    HealthCoordinator, MetricsRegistry, SqliteKvBindingExecutor, bind_binding_backend,
    serve_binding_backend,
};
use open_compute_storage::{
    PlatformStorage, QueueContentType, SchedulerStore, VersionState, WorkerRepository,
    WorkflowRepository,
};
use open_compute_workers::{
    BundleLimits, CanonicalBundle, CreateVersionOutcome, CreateVersionRequest, ModuleInput,
    ModuleType, ResourcePins, RuntimeSource, RuntimeValidator, VersionController, VersionPins,
};
use std::collections::BTreeMap;
use std::convert::Infallible;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

mod http;
mod nodejs;
mod wrangler;

mod p0_2_real_worker_create_validate_dispatch_promote_rollback_restart;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn p0_2_real_worker_create_validate_dispatch_promote_rollback_restart() {
    p0_2_real_worker_create_validate_dispatch_promote_rollback_restart::run().await;
}

async fn deploy_egress(
    controller: &VersionController<'_>,
    account: open_compute_core::AccountId,
    worker: open_compute_core::WorkerId,
    fixture: Option<&EgressFixture>,
) -> open_compute_storage::VersionRecord {
    let public_targets = fixture.map_or_else(Vec::new, |fixture| {
        vec![
            fixture.public_ipv4_url.clone(),
            fixture.public_ipv6_url.clone(),
            fixture.public_hostname_url.clone(),
        ]
    });
    let mut denied_targets = vec![
        "http://127.0.0.1:1/".to_owned(),
        "http://10.0.0.1/".to_owned(),
        "http://169.254.169.254/latest/meta-data/".to_owned(),
        "http://[::1]/".to_owned(),
        "http://[::ffff:127.0.0.1]/".to_owned(),
        "http://2130706433/".to_owned(),
        "http://user@127.0.0.1/".to_owned(),
        "http://localhost/".to_owned(),
        "file:///etc/passwd".to_owned(),
    ];
    if let Some(fixture) = fixture {
        denied_targets.push(fixture.redirect_private_url.clone());
        denied_targets.push(fixture.private_hostname_url.clone());
    }
    let mut vars = BTreeMap::new();
    vars.insert(
        "PUBLIC_TARGETS_JSON".to_owned(),
        serde_json::json!(serde_json::to_string(&public_targets).unwrap()),
    );
    vars.insert(
        "DENIED_TARGETS_JSON".to_owned(),
        serde_json::json!(serde_json::to_string(&denied_targets).unwrap()),
    );
    if let Some(fixture) = fixture {
        vars.insert(
            "RAW_TCP_CONFIG_JSON".to_owned(),
            serde_json::json!(
                serde_json::json!({
                    "ipv4Host": fixture.public_ipv4_host,
                    "ipv6Host": fixture.public_ipv6_host,
                    "hostname": fixture.public_hostname,
                    "privateHostname": fixture.private_hostname,
                    "tcpPort": fixture.public_tcp_port,
                    "tlsPort": fixture.public_tls_port,
                })
                .to_string()
            ),
        );
    }
    let bundle = CanonicalBundle::build(
        "index.js",
        vec![ModuleInput {
            name: "index.js".to_owned(),
            module_type: ModuleType::EsModule,
            bytes: include_bytes!("../../../../test/runtime/fixtures/p0-2-egress.js").to_vec(),
        }],
        BundleLimits::default(),
    )
    .unwrap();
    let request = CreateVersionRequest {
        account_id: account,
        worker_id: worker,
        idempotency_key: "deploy-egress".to_owned(),
        content: open_compute_workers::VersionContent::Worker {
            bundle: bundle.into_bytes().into(),
            assets: None,
        },
        vars,
        secrets: BTreeMap::new(),
        bindings: BTreeMap::new(),
        services: BTreeMap::new(),
        runtime_features: Default::default(),
        queue_consumers: Vec::new(),
        crons: vec!["3 * * * *".to_owned()],
        deployment_source: None,
        request_id: RequestId::generate(),
        now_ms: 20,
    };
    match controller.create_version(request).await.unwrap() {
        CreateVersionOutcome::Applied(result) => result.version,
        CreateVersionOutcome::Replay(_) => panic!("unexpected replay"),
    }
}

#[derive(Debug)]
struct EgressFixture {
    public_ipv4_url: String,
    public_ipv6_url: String,
    public_hostname_url: String,
    redirect_private_url: String,
    private_hostname_url: String,
    public_ipv4_host: String,
    public_ipv6_host: String,
    public_hostname: String,
    private_hostname: String,
    public_tcp_port: String,
    public_tls_port: String,
    tls_ca_path: PathBuf,
}

fn egress_fixture_from_env() -> Option<EgressFixture> {
    const NAMES: [&str; 12] = [
        "OPEN_COMPUTE_EGRESS_PUBLIC_IPV4_URL",
        "OPEN_COMPUTE_EGRESS_PUBLIC_IPV6_URL",
        "OPEN_COMPUTE_EGRESS_PUBLIC_HOSTNAME_URL",
        "OPEN_COMPUTE_EGRESS_REDIRECT_PRIVATE_URL",
        "OPEN_COMPUTE_EGRESS_PRIVATE_HOSTNAME_URL",
        "OPEN_COMPUTE_EGRESS_PUBLIC_IPV4_HOST",
        "OPEN_COMPUTE_EGRESS_PUBLIC_IPV6_HOST",
        "OPEN_COMPUTE_EGRESS_PUBLIC_HOSTNAME",
        "OPEN_COMPUTE_EGRESS_PRIVATE_HOSTNAME",
        "OPEN_COMPUTE_EGRESS_PUBLIC_TCP_PORT",
        "OPEN_COMPUTE_EGRESS_PUBLIC_TLS_PORT",
        "OPEN_COMPUTE_EGRESS_TLS_CA_PATH",
    ];
    let values = NAMES.map(std::env::var);
    if values.iter().all(Result::is_err) {
        return None;
    }
    let [
        public_ipv4_url,
        public_ipv6_url,
        public_hostname_url,
        redirect_private_url,
        private_hostname_url,
        public_ipv4_host,
        public_ipv6_host,
        public_hostname,
        private_hostname,
        public_tcp_port,
        public_tls_port,
        tls_ca_path,
    ] = values.map(|value| value.expect("all controlled egress fixture URLs must be set"));
    Some(EgressFixture {
        public_ipv4_url,
        public_ipv6_url,
        public_hostname_url,
        redirect_private_url,
        private_hostname_url,
        public_ipv4_host,
        public_ipv6_host,
        public_hostname,
        private_hostname,
        public_tcp_port,
        public_tls_port,
        tls_ca_path: PathBuf::from(tls_ca_path),
    })
}

async fn run_tls_fixture(workerd: &Path, root: &Path, fixture: &EgressFixture) {
    let temp = tempfile::tempdir().expect("TLS fixture tempdir");
    for name in ["p0-2-tls.wd-test", "p0-2-tls.js"] {
        std::fs::copy(
            root.join("test/runtime/fixtures").join(name),
            temp.path().join(name),
        )
        .expect("copy TLS fixture source");
    }
    std::fs::copy(&fixture.tls_ca_path, temp.path().join("ca.pem")).expect("copy TLS fixture CA");
    for test in ["cloudflareTlsOn", "cloudflareStartTls", "nodeTlsLifecycle"] {
        let mut command = tokio::process::Command::new(workerd);
        command
            .arg("test")
            .arg(temp.path().join("p0-2-tls.wd-test"))
            .arg("--experimental")
            .arg(format!("p0-2-tls:{test}"))
            .env(
                "OPEN_COMPUTE_EGRESS_PUBLIC_TLS_PORT",
                &fixture.public_tls_port,
            )
            .env(
                "OPEN_COMPUTE_EGRESS_PUBLIC_IPV4_HOST",
                &fixture.public_ipv4_host,
            )
            .kill_on_drop(true);
        let output = tokio::time::timeout(Duration::from_secs(15), command.output())
            .await
            .unwrap_or_else(|_| panic!("TLS fixture timed out: {test}"))
            .unwrap_or_else(|error| panic!("TLS fixture failed to start ({test}): {error}"));
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success(),
            "TLS fixture failed ({test}): {stderr}"
        );
        assert!(
            stderr.contains(&format!("[ PASS ] p0-2-tls:{test}")) && !stderr.contains("[ FAIL ]"),
            "TLS fixture did not report a clean pass ({test}): {stderr}"
        );
    }
}

fn assert_raw_tcp_fixture(raw: &serde_json::Value, fixture: &EgressFixture) {
    let sockets = &raw["sockets"];
    let expected_authorities = [
        (
            "ipv4",
            format!("{}:{}", fixture.public_ipv4_host, fixture.public_tcp_port),
        ),
        (
            "ipv6",
            format!("[{}]:{}", fixture.public_ipv6_host, fixture.public_tcp_port),
        ),
        (
            "dns",
            format!("{}:{}", fixture.public_hostname, fixture.public_tcp_port),
        ),
    ];
    for (name, expected_authority) in expected_authorities {
        assert_eq!(
            sockets[name]["bytes"],
            192 * 1024,
            "{name} socket echo: {}",
            sockets[name]
        );
        assert!(
            sockets[name]["chunks"]
                .as_u64()
                .is_some_and(|chunks| chunks > 1),
            "{name} socket echo must cross stream chunks: {}",
            sockets[name]
        );
        assert_eq!(
            sockets[name]["localAddress"],
            serde_json::Value::Null,
            "{name} outbound socket must not invent a local address"
        );
        assert_eq!(
            sockets[name]["remoteAddress"], expected_authority,
            "{name} outbound socket must preserve the requested authority"
        );
    }
    assert_eq!(
        sockets["ipv4"]["initialDesiredSize"], 4096,
        "highWaterMark must configure the writable stream"
    );
    assert_eq!(
        sockets["halfOpenFalse"]["marker"], "peer-half-close",
        "{}",
        sockets["halfOpenFalse"]
    );
    assert_eq!(sockets["halfOpenFalse"]["writeAfterEof"], false);
    assert_eq!(
        sockets["halfOpenTrue"]["marker"], "peer-half-close",
        "{}",
        sockets["halfOpenTrue"]
    );
    assert_eq!(sockets["halfOpenTrue"]["writeAfterEof"], true);
    assert_eq!(sockets["halfOpenTrue"]["closeError"], false);
    assert_eq!(
        sockets["tlsOn"]["certificateRejected"], true,
        "{}",
        sockets["tlsOn"]
    );
    assert_eq!(sockets["tlsOn"]["initialSecureTransport"], "on");
    assert_eq!(sockets["tlsOn"]["initialUpgraded"], false);
    assert_eq!(
        sockets["startTls"]["certificateRejected"], true,
        "{}",
        sockets["startTls"]
    );
    assert_eq!(sockets["startTls"]["initialSecureTransport"], "starttls");
    assert_eq!(sockets["startTls"]["initialUpgraded"], false);
    assert_eq!(sockets["startTls"]["oldSocketNeutered"], true);
    for name in ["privateDns", "loopback"] {
        assert_eq!(sockets[name]["opened"], false, "{name} raw socket");
        assert_eq!(sockets[name]["denied"], true, "{name} raw socket");
    }

    let node = &raw["node"];
    assert_eq!(
        node["net"]["bytes"],
        192 * 1024,
        "node net echo: {}",
        node["net"]
    );
    assert!(
        node["net"]["chunks"]
            .as_u64()
            .is_some_and(|chunks| chunks > 1),
        "node net echo must cross stream chunks: {}",
        node["net"]
    );
    assert_eq!(node["net"]["destroyed"], true);
    assert_eq!(node["tls"]["certificateRejected"], true, "{}", node["tls"]);
    assert_eq!(node["tls"]["errorEvent"], true, "{}", node["tls"]);
    assert_eq!(node["tls"]["destroyed"], true, "{}", node["tls"]);
    assert_eq!(node["timeout"]["timedOut"], true);
    assert_eq!(node["timeout"]["destroyed"], true);
    for name in ["privateDns", "loopback"] {
        assert_eq!(node[name]["opened"], false, "node {name}");
        assert_eq!(node[name]["denied"], true, "node {name}");
    }
}

async fn deploy_node(
    controller: &VersionController<'_>,
    account: open_compute_core::AccountId,
    worker: open_compute_core::WorkerId,
) -> open_compute_storage::VersionRecord {
    let bundle = CanonicalBundle::build(
        "index.js",
        vec![ModuleInput {
            name: "index.js".to_owned(),
            module_type: ModuleType::EsModule,
            bytes: br#"import { Buffer } from "node:buffer";
export default { fetch() { return new Response(Buffer.from("node-compat").toString()); } };"#
                .to_vec(),
        }],
        BundleLimits::default(),
    )
    .unwrap();
    let request = CreateVersionRequest {
        account_id: account,
        worker_id: worker,
        idempotency_key: "deploy-node-compat".to_owned(),
        content: open_compute_workers::VersionContent::Worker {
            bundle: bundle.into_bytes().into(),
            assets: None,
        },
        vars: BTreeMap::new(),
        secrets: BTreeMap::new(),
        bindings: BTreeMap::new(),
        services: BTreeMap::new(),
        runtime_features: Default::default(),
        queue_consumers: Vec::new(),
        crons: Vec::new(),
        deployment_source: None,
        request_id: RequestId::generate(),
        now_ms: 21,
    };
    match controller.create_version(request).await.unwrap() {
        CreateVersionOutcome::Applied(result) => result.version,
        CreateVersionOutcome::Replay(_) => panic!("unexpected replay"),
    }
}

async fn deploy(
    controller: &VersionController<'_>,
    account: open_compute_core::AccountId,
    worker: open_compute_core::WorkerId,
    key: &str,
    label: &str,
    promote: bool,
    invalid: bool,
) -> open_compute_storage::VersionRecord {
    match controller
        .create_version(create_request(
            account, worker, key, label, promote, invalid,
        ))
        .await
        .unwrap()
    {
        CreateVersionOutcome::Applied(result) => result.version,
        CreateVersionOutcome::Replay(_) => panic!("unexpected replay"),
    }
}

fn create_request(
    account: open_compute_core::AccountId,
    worker: open_compute_core::WorkerId,
    key: &str,
    label: &str,
    promote: bool,
    invalid: bool,
) -> CreateVersionRequest {
    let source = if invalid {
        "export default { fetch( {".to_owned()
    } else {
        format!(
            r#"import {{ WorkerEntrypoint }} from "cloudflare:workers";
export class Named extends WorkerEntrypoint {{
  async fetch(request) {{ return new Response("named:{label}:" + await request.text()); }}
  async queue(batch) {{
    if (batch.queue !== "runtime-gate" || batch.messages[0].body !== "named") throw new Error("named queue shape");
    batch.ackAll();
  }}
}}
export default {{
  async fetch(request, env) {{
    const path = new URL(request.url).pathname;
    if (path === "/runtime-gate/stream") return new Response(request.body);
    if (path === "/runtime-gate/early") return new Response("early-response");
    if (path === "/runtime-gate/midstream") return new Response(new ReadableStream({{
      start(controller) {{ controller.enqueue(new TextEncoder().encode("stream-prefix")); }},
      pull() {{ return new Promise(() => {{}}); }}
    }}));
    const content = await request.text();
    if (path === "/runtime-gate/path" && content === "conformance") {{
      const digest = await crypto.subtle.digest("SHA-256", new TextEncoder().encode("gate"));
      await new Promise((resolve) => setTimeout(resolve, 1));
      const stream = new ReadableStream({{ start(controller) {{ controller.close(); }} }});
      return Response.json({{
        fetch: typeof fetch === "function",
        request: typeof Request === "function",
        response: typeof Response === "function",
        headers: typeof Headers === "function",
        url: new URL("https://example.test/a").pathname === "/a",
        streams: stream instanceof ReadableStream,
        crypto: digest.byteLength === 32,
        timers: typeof setTimeout === "function",
        webSocket: typeof WebSocket === "function"
      }});
    }}
    return new Response("{label}:" + content + ":" + env.MODE + ":" + env.API_TOKEN + ":" + Object.keys(env).sort().join(","));
  }},
  async queue(batch, env, ctx) {{
    if (batch.queue === "runtime-gate-throw") throw new Error("queue failure");
    if (batch.queue === "runtime-gate-wait-until") {{
      ctx.waitUntil(Promise.reject(new Error("queue waitUntil failure")));
      return;
    }}
    if (batch.queue === "runtime-gate-timeout") await new Promise((resolve) => setTimeout(resolve, 10000));
    if (batch.queue !== "runtime-gate" || env.MODE !== "production" || batch.messages.length !== 3) throw new Error("queue shape");
    const [text, json, binary] = batch.messages;
    if (text.body !== "ack" || !(text.timestamp instanceof Date) || text.attempts !== 1) throw new Error("text shape");
    if (json.body.action !== "retry" || json.attempts !== 2) throw new Error("json shape");
    if (!(binary.body instanceof Uint8Array) || binary.body[1] !== 255 || binary.attempts !== 3) throw new Error("bytes shape");
    text.ack();
    text.retry({{ delaySeconds: 99 }});
    json.retry({{ delaySeconds: 7 }});
  }},
  async scheduled(controller, env, ctx) {{
    if (controller.cron === "1 * * * *") throw new Error("scheduled failure");
    if (controller.cron === "2 * * * *") {{
      ctx.waitUntil(Promise.reject(new Error("scheduled waitUntil failure")));
      return;
    }}
    if (controller.type !== "scheduled" || controller.cron !== "*/5 * * * *"
        || controller.scheduledTime !== 1787700060000 || env.MODE !== "production") throw new Error("scheduled shape");
    controller.noRetry();
  }}
}};"#
        )
    };
    let bundle = CanonicalBundle::build(
        "index.js",
        vec![ModuleInput {
            name: "index.js".to_owned(),
            module_type: ModuleType::EsModule,
            bytes: source.into_bytes(),
        }],
        BundleLimits::default(),
    )
    .unwrap();
    let mut vars = BTreeMap::new();
    vars.insert("MODE".to_owned(), serde_json::json!("production"));
    let mut secrets = BTreeMap::new();
    secrets.insert("API_TOKEN".to_owned(), SecretString::new("gate-secret"));
    CreateVersionRequest {
        account_id: account,
        worker_id: worker,
        idempotency_key: key.to_owned(),
        content: open_compute_workers::VersionContent::Worker {
            bundle: bundle.into_bytes().into(),
            assets: None,
        },
        vars,
        secrets,
        bindings: BTreeMap::new(),
        services: BTreeMap::new(),
        runtime_features: Default::default(),
        queue_consumers: Vec::new(),
        crons: vec![
            "*/5 * * * *".to_owned(),
            "1 * * * *".to_owned(),
            "2 * * * *".to_owned(),
        ],
        deployment_source: promote.then_some(open_compute_storage::DeploymentSource::ScriptUpload),
        request_id: RequestId::generate(),
        now_ms: 2,
    }
}

#[derive(Debug)]
struct DispatchResponse {
    status: u16,
    body: String,
    loader_outcome: Option<LoaderOutcome>,
}

struct PendingUpload {
    first: Option<Bytes>,
    dropped: Arc<AtomicBool>,
}

impl Stream for PendingUpload {
    type Item = Result<Bytes, Infallible>;

    fn poll_next(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match self.first.take() {
            Some(bytes) => Poll::Ready(Some(Ok(bytes))),
            None => Poll::Pending,
        }
    }
}

impl Drop for PendingUpload {
    fn drop(&mut self) {
        self.dropped.store(true, Ordering::SeqCst);
    }
}

fn dispatch_target(
    account: open_compute_core::AccountId,
    worker: open_compute_core::WorkerId,
    version: &open_compute_storage::VersionRecord,
    entrypoint: Option<&str>,
) -> DispatchTarget {
    DispatchTarget {
        account_id: account,
        worker_id: worker,
        version_id: version.id,
        worker_code_sha256: hex::encode(version.worker_code_sha256),
        entrypoint: entrypoint.map(str::to_owned),
        route_generation: 1,
        request_id: RequestId::generate(),
    }
}

async fn dispatch(
    transport: &WorkerdTransport,
    account: open_compute_core::AccountId,
    worker: open_compute_core::WorkerId,
    version: &open_compute_storage::VersionRecord,
    entrypoint: Option<&str>,
    body: &str,
) -> DispatchResponse {
    let request = Request::builder()
        .method("POST")
        .uri("/runtime-gate/path?x=1")
        .header(header::HOST, "workers.example.test")
        .header("x-open-compute-account-id", "forged")
        .body(Body::from(body.to_owned()))
        .unwrap();
    let response = transport
        .dispatch(
            dispatch_target(account, worker, version, entrypoint),
            request,
        )
        .await
        .unwrap();
    let status = response.status().as_u16();
    let loader_outcome = response.extensions().get::<LoaderOutcome>().copied();
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    DispatchResponse {
        status,
        body: String::from_utf8(bytes.to_vec()).unwrap(),
        loader_outcome,
    }
}

async fn wait_running(supervisor: &WorkerdSupervisor, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    let mut rx = supervisor.subscribe();
    loop {
        let snapshot = rx.borrow().clone();
        if snapshot.state == SupervisorState::Running {
            return;
        }
        assert!(
            snapshot.state != SupervisorState::Failed,
            "supervisor failed: {snapshot:?}; diagnostics={:?}",
            supervisor.last_diagnostics()
        );
        assert!(Instant::now() < deadline, "supervisor did not become ready");
        tokio::time::timeout(Duration::from_millis(250), rx.changed())
            .await
            .ok();
    }
}

async fn wait_pid_change(supervisor: &WorkerdSupervisor, previous: i32, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    let mut rx = supervisor.subscribe();
    loop {
        let snapshot = rx.borrow().clone();
        if snapshot.state == SupervisorState::Running && snapshot.pid != Some(previous) {
            return;
        }
        assert!(Instant::now() < deadline, "supervisor did not restart");
        tokio::time::timeout(Duration::from_millis(250), rx.changed())
            .await
            .ok();
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

fn storage_config(root: &Path) -> DataConfig {
    DataConfig {
        path: root.to_owned(),
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
request_timeout_ms = 3000
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
    ArtifactStore::new(ObjectBackend::connect_s3(&config, &credentials, 32 * 1024 * 1024).unwrap())
}

pub(crate) const ADMIN_TOKEN: &str = "p0-2-admin-secret";

pub(crate) fn write_secret(path: &Path, value: &str) -> PathBuf {
    std::fs::write(path, format!("{value}\n")).expect("write HTTP token");
    let mut permissions = std::fs::metadata(path)
        .expect("HTTP token metadata")
        .permissions();
    permissions.set_mode(0o600);
    std::fs::set_permissions(path, permissions).expect("HTTP token permissions");
    path.to_owned()
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_owned()
}
