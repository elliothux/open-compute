//! Real pinned-workerd P0.3 resource-binding framework Gate.
//! Kept as one cohesive matrix so all RB assertions share one generation,
//! immutable version chain, restart, and final leak audit.

#![cfg(feature = "test-support")]

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::TokioExecutor;
use open_compute_artifacts::{
    ArtifactStore, MapEnv, MockS3, ObjectBackend, resolve_s3_credentials_with,
};
use open_compute_core::clock::SystemClock;
use open_compute_core::config::{DataConfig, PlatformConfig, RuntimeConfig};
use open_compute_core::{
    AccountId, BindingKind, CanonicalBindingConfig, CanonicalPermissions, ErrorCode, PlatformError,
    Redactor, RequestId, ResourceAvailability, ResourceId, ResourceState,
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
    KvBindingExecutor, KvCommand, KvCommandResult, KvStreamPart, bind_binding_backend,
    serve_binding_backend,
};
use open_compute_storage::{
    AuthorizedBinding, BindingRepository, PlatformStorage, ResourceRecord, ResourceRepository,
    VersionRecord, WorkerRepository,
};
use open_compute_workers::{
    BundleLimits, CanonicalBundle, CreateResourceOutcome, CreateResourceRequest,
    CreateVersionOutcome, CreateVersionRequest, ModuleInput, ModuleType, ReconcileOutcome,
    ResourceController, ResourceDriver, ResourceHealth, ResourcePins, RuntimeSource,
    RuntimeValidator, VersionBindingInput, VersionController,
};
use rusqlite::params;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Debug, Default)]
struct FakeState {
    values: Mutex<HashMap<ResourceId, HashMap<String, Vec<u8>>>>,
    unavailable: Mutex<HashSet<ResourceId>>,
    deleted: Mutex<HashSet<ResourceId>>,
}

#[derive(Clone, Debug)]
struct FakeDriver(Arc<FakeState>);

impl ResourceDriver for FakeDriver {
    fn kind(&self) -> BindingKind {
        BindingKind::KvNamespace
    }

    fn create(&self, resource: &ResourceRecord) -> Result<(), PlatformError> {
        self.0
            .values
            .lock()
            .unwrap()
            .entry(resource.id)
            .or_default();
        Ok(())
    }

    fn reconcile(&self, resource: &ResourceRecord) -> Result<ReconcileOutcome, PlatformError> {
        if self.0.deleted.lock().unwrap().contains(&resource.id) {
            Ok(ReconcileOutcome::Deleted)
        } else if self.0.values.lock().unwrap().contains_key(&resource.id) {
            Ok(ReconcileOutcome::Ready)
        } else {
            Ok(ReconcileOutcome::Absent)
        }
    }

    fn begin_delete(&self, resource: &ResourceRecord) -> Result<(), PlatformError> {
        self.0.values.lock().unwrap().remove(&resource.id);
        self.0.deleted.lock().unwrap().insert(resource.id);
        Ok(())
    }

    fn finalize_delete(&self, resource: &ResourceRecord) -> Result<(), PlatformError> {
        if self.0.deleted.lock().unwrap().contains(&resource.id) {
            Ok(())
        } else {
            Err(PlatformError::new(
                ErrorCode::ResourceInvariantViolation,
                "fake resource delete did not begin",
            ))
        }
    }

    fn health(&self, resource: &ResourceRecord) -> Result<ResourceHealth, PlatformError> {
        if self.0.unavailable.lock().unwrap().contains(&resource.id) {
            Ok(ResourceHealth {
                availability: ResourceAvailability::Unavailable,
                code: Some("FAKE_UNAVAILABLE"),
            })
        } else {
            Ok(ResourceHealth::healthy())
        }
    }
}

#[derive(Clone, Debug)]
struct FakeExecutor(Arc<FakeState>);

impl KvBindingExecutor for FakeExecutor {
    fn execute(
        &self,
        binding: &AuthorizedBinding,
        command: KvCommand,
    ) -> Result<KvCommandResult, PlatformError> {
        self.ensure_available(binding.resource.id)?;
        let mut resources = self.0.values.lock().unwrap();
        let values = resources
            .get_mut(&binding.resource.id)
            .ok_or_else(missing)?;
        match command {
            KvCommand::Get { keys, .. } => Ok(KvCommandResult::Entries(
                keys.iter()
                    .map(|key| {
                        values.get(key).map(|value| open_compute_storage::KvEntry {
                            value: value.clone(),
                            metadata_json: None,
                            expires_at_ms: None,
                        })
                    })
                    .collect(),
            )),
            KvCommand::Put { key, value, .. } => {
                values.insert(key, value);
                Ok(KvCommandResult::Mutation)
            }
            KvCommand::PutStaged { key, mut value, .. } => {
                values.insert(key, value.read_all_for_test()?);
                Ok(KvCommandResult::Mutation)
            }
            KvCommand::Delete { key } => {
                values.remove(&key);
                Ok(KvCommandResult::Mutation)
            }
            KvCommand::List { .. } => {
                unreachable!("resource binding fixture does not list KV keys")
            }
        }
    }

    fn stream_get(
        &self,
        binding: &AuthorizedBinding,
        key: &str,
        _: Option<u64>,
        sink: &mut dyn FnMut(KvStreamPart) -> Result<(), PlatformError>,
    ) -> Result<(), PlatformError> {
        self.ensure_available(binding.resource.id)?;
        let value = self
            .0
            .values
            .lock()
            .unwrap()
            .get(&binding.resource.id)
            .and_then(|values| values.get(key).cloned());
        sink(KvStreamPart::Entry(value.as_ref().map(|value| {
            open_compute_storage::KvEntryInfo {
                value_length: value.len(),
                metadata_json: None,
                expires_at_ms: None,
            }
        })))?;
        if let Some(value) = value {
            for chunk in value.chunks(64 * 1024) {
                sink(KvStreamPart::Bytes(chunk.to_vec()))?;
            }
        }
        Ok(())
    }
}

impl FakeExecutor {
    fn ensure_available(&self, resource_id: ResourceId) -> Result<(), PlatformError> {
        if self.0.unavailable.lock().unwrap().contains(&resource_id) {
            return Err(PlatformError::new(
                ErrorCode::ResourceUnavailable,
                "fake resource is unavailable",
            ));
        }
        Ok(())
    }
}

mod p0_3_real_binding_matrix;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn p0_3_real_binding_matrix() {
    p0_3_real_binding_matrix::run().await;
}

fn create_resource<D: ResourceDriver>(
    controller: &ResourceController<'_, D>,
    account: AccountId,
    name: &str,
    key: &str,
    now_ms: i64,
) -> ResourceId {
    match controller
        .create(&resource_request(account, name, key, now_ms))
        .unwrap()
    {
        CreateResourceOutcome::Applied(result) => {
            assert_eq!(result.state, ResourceState::Ready);
            result.resource_id
        }
        CreateResourceOutcome::Replay(_) => panic!("unexpected resource replay"),
    }
}

fn resource_request(
    account_id: AccountId,
    name: &str,
    key: &str,
    now_ms: i64,
) -> CreateResourceRequest {
    CreateResourceRequest {
        account_id,
        kind: BindingKind::KvNamespace,
        name: name.to_owned(),
        idempotency_key: key.to_owned(),
        driver_schema_version: 1,
        request_id: RequestId::generate(),
        now_ms,
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
    key: &str,
    binding: Option<(ResourceId, CanonicalPermissions)>,
    promote: bool,
    collision: bool,
    now_ms: i64,
) -> CreateVersionRequest {
    let source = r#"export default {
  async fetch(request, env) {
    const path = new URL(request.url).pathname;
    if (path === "/put") { await env.KV.put("gate", await request.text()); return new Response("put"); }
    if (path === "/get") return new Response((await env.KV.get("gate")) ?? "null");
    if (path === "/stream") { await env.KV.put("stream", request.body); return new Response(await env.KV.get("stream", "stream")); }
    if (path === "/props") return Response.json({ own: Reflect.ownKeys(env.KV).map(String), backend: "BINDING_BACKEND" in env });
    return new Response("plain");
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
    let mut vars = BTreeMap::new();
    if collision {
        vars.insert("KV".to_owned(), serde_json::json!("collision"));
    }
    let mut bindings = BTreeMap::new();
    if let Some((resource_id, permissions)) = binding {
        bindings.insert(
            "KV".to_owned(),
            VersionBindingInput {
                kind: BindingKind::KvNamespace,
                id: resource_id,
                permissions,
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
    path: &str,
    body: &str,
) -> DispatchResponse {
    let request = Request::builder()
        .method("POST")
        .uri(path)
        .header(header::HOST, "binding.test")
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

#[allow(
    clippy::too_many_arguments,
    reason = "scenario helpers keep distinct fixture identities explicit"
)]
async fn backend_call(
    address: std::net::SocketAddr,
    token: &str,
    generation: &str,
    binding_id: open_compute_core::BindingId,
    version_id: open_compute_core::VersionId,
    descriptor_sha256: &str,
    operation: &str,
    body: &[u8],
    content_length: Option<usize>,
) -> hyper::Response<hyper::body::Incoming> {
    let mut request = Request::builder()
        .method("POST")
        .uri(format!(
            "http://{address}/internal/bindings/v1/kv/{binding_id}/{operation}"
        ))
        .header("content-type", "application/vnd.open-compute.kv.v1+frame")
        .header("x-open-compute-binding-token", token)
        .header("x-open-compute-startup-generation", generation)
        .header("x-open-compute-version-id", version_id.to_string())
        .header("x-open-compute-descriptor-sha256", descriptor_sha256)
        .header(
            "x-open-compute-request-id",
            RequestId::generate().to_string(),
        );
    if let Some(length) = content_length {
        request = request.header(header::CONTENT_LENGTH, length);
    }
    let client: Client<HttpConnector, Body> =
        Client::builder(TokioExecutor::new()).build(HttpConnector::new());
    client
        .request(request.body(Body::from(body.to_vec())).unwrap())
        .await
        .unwrap()
}

fn insert_account(path: PathBuf, account_id: AccountId) {
    let connection = rusqlite::Connection::open(path).unwrap();
    connection
        .execute(
            "INSERT INTO accounts (id, name, created_at_ms, deleted_at_ms) VALUES (?1, ?2, 1, NULL)",
            params![account_id.to_string(), format!("foreign-{account_id}")],
        )
        .unwrap();
}

fn tamper_descriptor(path: PathBuf, binding_id: open_compute_core::BindingId, digest: [u8; 32]) {
    let connection = rusqlite::Connection::open(path).unwrap();
    connection
        .execute_batch("DROP TRIGGER IF EXISTS version_bindings_update_guard")
        .unwrap();
    connection
        .execute(
            "UPDATE version_bindings SET descriptor_sha256 = ?1 WHERE id = ?2",
            params![digest.as_slice(), binding_id.to_string()],
        )
        .unwrap();
}

fn delete_version(
    repository: WorkerRepository<'_>,
    account_id: AccountId,
    worker_id: open_compute_core::WorkerId,
    version_id: open_compute_core::VersionId,
    now_ms: i64,
) {
    repository
        .begin_version_delete(account_id, worker_id, version_id)
        .unwrap();
    repository
        .finalize_version_delete(
            account_id,
            worker_id,
            version_id,
            RequestId::generate(),
            now_ms,
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

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_owned()
}

fn missing() -> PlatformError {
    PlatformError::new(
        ErrorCode::ResourceInvariantViolation,
        "fake physical resource is missing",
    )
}
