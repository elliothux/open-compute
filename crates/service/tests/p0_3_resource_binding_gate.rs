//! Real pinned-workerd P0.3 resource-binding framework Gate.
//! Kept as one cohesive matrix so all RB assertions share one generation,
//! immutable deployment chain, restart, and final leak audit.

#![cfg(feature = "test-support")]

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::TokioExecutor;
use open_compute_artifacts::{
    ArtifactStore, MapEnv, MockS3, S3ArtifactClient, resolve_s3_credentials_with,
};
use open_compute_core::clock::SystemClock;
use open_compute_core::config::{PlatformConfig, RuntimeConfig, StorageConfig};
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
use open_compute_service::{KvBindingExecutor, bind_binding_backend, serve_binding_backend};
use open_compute_storage::{
    AuthorizedBinding, BindingRepository, DeploymentRecord, PlatformStorage, ResourceRecord,
    ResourceRepository, WorkerRepository,
};
use open_compute_workers::{
    BundleLimits, CanonicalBundle, CreateDeploymentOutcome, CreateDeploymentRequest,
    CreateResourceOutcome, CreateResourceRequest, DeploymentBindingInput, DeploymentController,
    ModuleInput, ModuleType, ReconcileOutcome, ResourceController, ResourceDriver, ResourceHealth,
    ResourcePins, RuntimeSource, RuntimeValidator,
};
use rusqlite::params;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Debug, Default)]
struct FakeState {
    values: Mutex<HashMap<ResourceId, HashMap<String, String>>>,
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
    fn get(&self, binding: &AuthorizedBinding, key: &str) -> Result<Option<String>, PlatformError> {
        self.ensure_available(binding.resource.id)?;
        Ok(self
            .0
            .values
            .lock()
            .unwrap()
            .get(&binding.resource.id)
            .and_then(|values| values.get(key).cloned()))
    }

    fn put(
        &self,
        binding: &AuthorizedBinding,
        key: &str,
        value: &str,
    ) -> Result<(), PlatformError> {
        self.ensure_available(binding.resource.id)?;
        let mut resources = self.0.values.lock().unwrap();
        let values = resources
            .get_mut(&binding.resource.id)
            .ok_or_else(missing)?;
        values.insert(key.to_owned(), value.to_owned());
        Ok(())
    }

    fn delete(&self, binding: &AuthorizedBinding, key: &str) -> Result<(), PlatformError> {
        self.ensure_available(binding.resource.id)?;
        let mut resources = self.0.values.lock().unwrap();
        let values = resources
            .get_mut(&binding.resource.id)
            .ok_or_else(missing)?;
        values.remove(key);
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn p0_3_real_binding_matrix() {
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
    let fake = Arc::new(FakeState::default());
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
        let storage = storage.clone();
        let auth = binding_auth.clone();
        let pins = pins.clone();
        let executor = Arc::new(FakeExecutor(fake.clone()));
        async move {
            serve_binding_backend(
                binding_listener,
                storage,
                auth,
                pins,
                executor,
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
            version: "p0.3-gate".to_owned(),
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
    let supervisor = Arc::new(WorkerdSupervisor::new_with_services_and_auth(
        WorkerdSupervisorOptions {
            runtime,
            compiler,
            config: runtime_config(workerd, lock, root.join("runtime")),
            clock: Arc::new(SystemClock),
            jitter: Arc::new(OsJitter),
            redactor: Redactor::new(),
            lease_path: Some(storage.data_dir().runtime_dir().join("p0-3-gate.lease")),
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
    let controller = ResourceController::new(&storage, pins.clone(), FakeDriver(fake.clone()));
    let resource = create_resource(&controller, account, "cache", "resource-create", 10);
    assert!(matches!(
        controller
            .create(&resource_request(account, "cache", "resource-create", 11))
            .unwrap(),
        CreateResourceOutcome::Replay(bytes) if !bytes.is_empty()
    ));
    let repository = WorkerRepository::new(storage.db());
    let (worker, _) = repository
        .create_worker(account, "binding-gate", RequestId::generate(), 12)
        .unwrap();
    let validator: Arc<dyn RuntimeValidator> = Arc::new(transport.clone());
    let deployments = DeploymentController::new(
        &storage,
        artifacts.clone(),
        validator,
        BundleLimits::default(),
    );

    let collision = deployments
        .create_deployment(deployment_request(
            account,
            worker.id,
            "env-collision",
            Some((resource, CanonicalPermissions::default())),
            true,
            true,
            20,
        ))
        .await
        .unwrap_err();
    assert_eq!(collision.code(), ErrorCode::BindingTypeMismatch);

    let foreign = AccountId::generate();
    insert_account(storage.data_dir().control_db_path(), foreign);
    let (foreign_worker, _) = repository
        .create_worker(foreign, "foreign", RequestId::generate(), 21)
        .unwrap();
    let cross_account = deployments
        .create_deployment(deployment_request(
            foreign,
            foreign_worker.id,
            "cross-account",
            Some((resource, CanonicalPermissions::default())),
            false,
            false,
            22,
        ))
        .await
        .unwrap_err();
    assert_eq!(cross_account.code(), ErrorCode::ResourceNotFound);

    let bound = deploy(
        &deployments,
        deployment_request(
            account,
            worker.id,
            "bound",
            Some((resource, CanonicalPermissions::default())),
            true,
            false,
            30,
        ),
    )
    .await;
    let put = dispatch(&transport, account, worker.id, &bound, "/put", "alpha").await;
    assert_eq!(put.status, 200, "{}", put.body);
    assert_eq!(put.loader_outcome, Some(LoaderOutcome::Cold));
    let get = dispatch(&transport, account, worker.id, &bound, "/get", "").await;
    assert_eq!((get.status, get.body.as_str()), (200, "alpha"));
    assert_eq!(get.loader_outcome, Some(LoaderOutcome::Warm));

    ResourceRepository::new(storage.db())
        .rename(
            account,
            resource,
            "renamed-cache",
            RequestId::generate(),
            31,
        )
        .unwrap();
    let renamed = dispatch(&transport, account, worker.id, &bound, "/get", "").await;
    assert_eq!(renamed.body, "alpha");
    let props = dispatch(&transport, account, worker.id, &bound, "/props", "").await;
    assert_eq!(props.status, 200);
    assert!(!props.body.contains(&resource.to_string()));
    assert!(!props.body.contains("BINDING_BACKEND"));
    let echoed = dispatch(&transport, account, worker.id, &bound, "/echo", "stream-ok").await;
    assert_eq!((echoed.status, echoed.body.as_str()), (200, "stream-ok"));
    assert_eq!(pins.count(resource), 0);

    let binding = BindingRepository::new(storage.db())
        .deployment_bindings(bound.id)
        .unwrap()
        .pop()
        .unwrap();
    let current_token = binding_auth.credential().unwrap();
    let generation = binding_auth.claimed_generation_for_test().unwrap();
    let forged_token = backend_call(
        binding_addr,
        &"00".repeat(32),
        &generation,
        binding.id,
        bound.id,
        &hex::encode(binding.descriptor_sha256),
        "get",
        br#"{"key":"k"}"#,
        None,
    )
    .await;
    assert_eq!(forged_token.status(), StatusCode::NOT_FOUND);
    assert!(
        forged_token
            .headers()
            .get("x-open-compute-error-code")
            .is_none()
    );
    let forged_hash = backend_call(
        binding_addr,
        current_token.expose(),
        &generation,
        binding.id,
        bound.id,
        &"00".repeat(32),
        "get",
        br#"{"key":"k"}"#,
        None,
    )
    .await;
    assert_eq!(
        forged_hash
            .headers()
            .get("x-open-compute-error-code")
            .unwrap(),
        ErrorCode::BindingTypeMismatch.as_str()
    );
    let oversized = backend_call(
        binding_addr,
        current_token.expose(),
        &generation,
        binding.id,
        bound.id,
        &hex::encode(binding.descriptor_sha256),
        "put",
        b"",
        Some(1024 * 1024 + 1),
    )
    .await;
    assert_eq!(oversized.status(), StatusCode::PAYLOAD_TOO_LARGE);

    let original_hash = binding.descriptor_sha256;
    tamper_descriptor(storage.data_dir().control_db_path(), binding.id, [0; 32]);
    let warm_tamper = dispatch(&transport, account, worker.id, &bound, "/get", "").await;
    assert_eq!(warm_tamper.status, 500);
    assert!(warm_tamper.body.contains("DEPLOYMENT_INVARIANT_VIOLATION"));
    assert_eq!(
        repository
            .get_worker(account, worker.id)
            .unwrap()
            .active_deployment_id,
        Some(bound.id)
    );
    tamper_descriptor(
        storage.data_dir().control_db_path(),
        binding.id,
        original_hash,
    );

    let read_only = deploy(
        &deployments,
        deployment_request(
            account,
            worker.id,
            "read-only",
            Some((
                resource,
                CanonicalPermissions {
                    read: true,
                    write: false,
                },
            )),
            true,
            false,
            40,
        ),
    )
    .await;
    let denied = dispatch(&transport, account, worker.id, &read_only, "/put", "denied").await;
    assert_eq!(denied.status, 500);
    assert!(denied.body.contains("BINDING_PERMISSION_DENIED"));

    fake.values
        .lock()
        .unwrap()
        .get_mut(&resource)
        .unwrap()
        .insert("gate".to_owned(), "x".repeat(1024 * 1024 + 1));
    let result_limit = dispatch(&transport, account, worker.id, &read_only, "/get", "").await;
    assert_eq!(result_limit.status, 500);
    assert!(result_limit.body.contains("BINDING_LIMIT_EXCEEDED"));
    fake.values
        .lock()
        .unwrap()
        .get_mut(&resource)
        .unwrap()
        .insert("gate".to_owned(), "alpha".to_owned());

    ResourceRepository::new(storage.db())
        .set_availability(
            account,
            resource,
            ResourceAvailability::Unavailable,
            Some("FAKE_UNAVAILABLE"),
            41,
        )
        .unwrap();
    let isolated = dispatch(&transport, account, worker.id, &read_only, "/get", "").await;
    assert_eq!(isolated.status, 500);
    assert!(isolated.body.contains("RESOURCE_UNAVAILABLE"));
    ResourceRepository::new(storage.db())
        .set_availability(account, resource, ResourceAvailability::Healthy, None, 42)
        .unwrap();

    let delete_referenced = controller
        .delete(
            account,
            resource,
            RequestId::generate(),
            43,
            Duration::from_millis(50),
        )
        .await
        .unwrap_err();
    assert_eq!(delete_referenced.code(), ErrorCode::ResourceReferenced);

    let old_pid = supervisor.snapshot().pid.unwrap();
    let old_token = current_token.expose().to_owned();
    supervisor.report_unhealthy();
    wait_pid_change(&supervisor, old_pid, Duration::from_secs(30)).await;
    let stale = backend_call(
        binding_addr,
        &old_token,
        &generation,
        binding.id,
        bound.id,
        &hex::encode(original_hash),
        "get",
        br#"{"key":"gate"}"#,
        None,
    )
    .await;
    assert_eq!(stale.status(), StatusCode::NOT_FOUND);
    let post_restart = dispatch(&transport, account, worker.id, &read_only, "/get", "").await;
    assert_eq!(post_restart.body, "alpha");

    let held = pins.try_pin(resource).unwrap();
    let drain = pins.fence_and_wait(resource, Duration::from_secs(1));
    tokio::pin!(drain);
    assert!(
        tokio::time::timeout(Duration::from_millis(5), &mut drain)
            .await
            .is_err()
    );
    drop(held);
    drain.await.unwrap();
    pins.unfence(resource);

    let plain = deploy(
        &deployments,
        deployment_request(account, worker.id, "plain", None, true, false, 50),
    )
    .await;
    let plain_result = dispatch(&transport, account, worker.id, &plain, "/plain", "").await;
    assert_eq!(
        (plain_result.status, plain_result.body.as_str()),
        (200, "plain")
    );
    repository
        .prune_expired_idempotency(24 * 60 * 60 * 1000 + 100, 100)
        .unwrap();
    delete_deployment(repository, account, worker.id, bound.id, 51);
    delete_deployment(repository, account, worker.id, read_only.id, 52);
    controller
        .delete(
            account,
            resource,
            RequestId::generate(),
            53,
            Duration::from_secs(1),
        )
        .await
        .unwrap();
    assert_eq!(pins.count(resource), 0);
    let recreated = create_resource(&controller, account, "renamed-cache", "recreate", 54);
    assert_ne!(recreated, resource);

    let diagnostics = format!("{:?}", supervisor.last_diagnostics());
    assert!(!diagnostics.contains(&old_token));
    supervisor.shutdown().await;
    assert_eq!(supervisor.owner_registry_len(), 0);
    let _ = shutdown_tx.send(true);
    source_task.await.unwrap().unwrap();
    binding_task.await.unwrap().unwrap();
    assert_eq!(pins.count(recreated), 0);
    println!("RB-01..RB-18 PASS");
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
    key: &str,
    binding: Option<(ResourceId, CanonicalPermissions)>,
    promote: bool,
    collision: bool,
    now_ms: i64,
) -> CreateDeploymentRequest {
    let source = r#"export default {
  async fetch(request, env) {
    const path = new URL(request.url).pathname;
    if (path === "/put") { await env.KV.put("gate", await request.text()); return new Response("put"); }
    if (path === "/get") return new Response((await env.KV.get("gate")) ?? "null");
    if (path === "/echo") return new Response(await env.KV.echoStream(request.body));
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
            DeploymentBindingInput {
                kind: BindingKind::KvNamespace,
                id: resource_id,
                permissions,
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
        limits: serde_json::json!({"profile":"default"}),
        promote,
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
    deployment: &DeploymentRecord,
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

#[allow(clippy::too_many_arguments)]
async fn backend_call(
    address: std::net::SocketAddr,
    token: &str,
    generation: &str,
    binding_id: open_compute_core::BindingId,
    deployment_id: open_compute_core::DeploymentId,
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
        .header("content-type", "application/vnd.open-compute.kv.v1+json")
        .header("x-open-compute-binding-token", token)
        .header("x-open-compute-startup-generation", generation)
        .header("x-open-compute-deployment-id", deployment_id.to_string())
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
        .execute_batch("DROP TRIGGER IF EXISTS deployment_bindings_update_guard")
        .unwrap();
    connection
        .execute(
            "UPDATE deployment_bindings SET descriptor_sha256 = ?1 WHERE id = ?2",
            params![digest.as_slice(), binding_id.to_string()],
        )
        .unwrap();
}

fn delete_deployment(
    repository: WorkerRepository<'_>,
    account_id: AccountId,
    worker_id: open_compute_core::WorkerId,
    deployment_id: open_compute_core::DeploymentId,
    now_ms: i64,
) {
    repository
        .begin_deployment_delete(account_id, worker_id, deployment_id)
        .unwrap();
    repository
        .finalize_deployment_delete(
            account_id,
            worker_id,
            deployment_id,
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

fn runtime_config(binary: PathBuf, lock: PathBuf, assets: PathBuf) -> RuntimeConfig {
    RuntimeConfig {
        binary,
        lock_file: lock,
        assets_dir: assets,
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

fn missing() -> PlatformError {
    PlatformError::new(
        ErrorCode::ResourceInvariantViolation,
        "fake physical resource is missing",
    )
}
