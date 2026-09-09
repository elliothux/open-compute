//! Real pinned-workerd P0.7 Durable Objects identity, facet, lifecycle, and restart Gate.
//!
//! This intentionally stays one cohesive process matrix so one fixture proves identity,
//! version fencing, native persistence, `WebSockets`, and destructive lifecycle together.

#![cfg(feature = "test-support")]

use axum::body::{Body, to_bytes};
use axum::http::{Request, header};
use hmac::{Hmac, Mac};
use open_compute_artifacts::{
    ArtifactStore, MapEnv, MockS3, ObjectBackend, resolve_s3_credentials_with,
};
use open_compute_core::clock::SystemClock;
use open_compute_core::config::{DataConfig, DurableObjectsConfig, PlatformConfig, RuntimeConfig};
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
    DO_NAMESPACE_SCHEMA_VERSION, DurableObjectRepository, PlatformStorage, VersionRecord,
    WorkerRepository,
};
use open_compute_workers::{
    BundleLimits, CanonicalBundle, CreateResourceOutcome, CreateResourceRequest,
    CreateVersionOutcome, CreateVersionRequest, DurableObjectResourceDriver, ModuleInput,
    ModuleType, ResourceController, ResourcePins, RuntimeSource, RuntimeValidator,
    VersionBindingInput, VersionContent, VersionController,
};
use sha2::Sha256;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[path = "../../../../test/runtime/durable-objects/hibernation.rs"]
mod hibernation;
#[path = "../../../../test/runtime/durable-objects/output_crash.rs"]
mod output_crash;
#[path = "../../../../test/runtime/durable-objects/recovery.rs"]
mod recovery;

mod p0_7_real_durable_objects_matrix;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn p0_7_real_durable_objects_matrix() {
    p0_7_real_durable_objects_matrix::run().await;
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

#[allow(
    clippy::too_many_arguments,
    reason = "scenario helpers keep distinct fixture identities explicit"
)]
fn version_request(
    account_id: AccountId,
    worker_id: WorkerId,
    counter: ResourceId,
    other: ResourceId,
    output_queue: ResourceId,
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
            bytes: include_str!("../../../../test/runtime/fixtures/durable-objects/counter.js")
                .as_bytes()
                .to_vec(),
        }],
        BundleLimits::default(),
    )
    .unwrap();
    let mut bindings = BTreeMap::new();
    for (name, id) in [("COUNTER", counter), ("OTHER", other)] {
        bindings.insert(
            name.to_owned(),
            VersionBindingInput {
                kind: BindingKind::DoNamespace,
                id,
                permissions: CanonicalPermissions::default(),
                config: CanonicalBindingConfig::default(),
            },
        );
    }
    bindings.insert(
        "EVENTS".to_owned(),
        VersionBindingInput {
            kind: BindingKind::QueueProducer,
            id: output_queue,
            permissions: CanonicalPermissions::default(),
            config: CanonicalBindingConfig::default(),
        },
    );
    let mut vars = BTreeMap::new();
    vars.insert("RELEASE".to_owned(), serde_json::json!(release));
    CreateVersionRequest {
        account_id,
        worker_id,
        idempotency_key: key.to_owned(),
        content: VersionContent::Worker {
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

#[derive(Debug)]
struct DispatchResponse {
    status: u16,
    body: String,
}

fn assert_rpc_capability(response: &DispatchResponse, release: &str) {
    assert_eq!(response.status, 200, "{}", response.body);
    let value: serde_json::Value = serde_json::from_str(&response.body).unwrap();
    assert_eq!(value["direct"], format!("{release}:ok"));
    assert_eq!(value["property"], format!("{release}:capability"));
    assert_eq!(value["nested"], format!("{release}:nested:ok"));
    assert_eq!(value["envelope"], format!("{release}:ok"));
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
        .header(header::HOST, "do.test")
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
        .unwrap_or_else(|error| panic!("dispatch {path} failed: {error:?}"));
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
            snapshot.state != SupervisorState::Failed && start.elapsed() < timeout,
            "runtime did not restart: {snapshot:?}; diagnostics={:?}",
            supervisor.last_diagnostics()
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
    // This cohesive recovery matrix intentionally kills one runtime generation
    // for each independent crash boundary it verifies.
    config.restart_budget = 12;
    config
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .unwrap()
        .to_path_buf()
}
