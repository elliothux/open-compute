//! Shared real-workerd Service Binding harness.

use open_compute_artifacts::{
    ArtifactCache, ArtifactStore, MapEnv, MockS3, S3ArtifactClient, resolve_s3_credentials_with,
};
use open_compute_core::{
    CacheConfig, DurableObjectsConfig, PlatformConfig, Redactor, RuntimeConfig, StartupId,
    StorageConfig, SystemClock,
};
use open_compute_runtime::{
    DirectoryServicePath, ExternalServiceAddress, GenerationAuthRegistry, OsJitter,
    PlatformReleaseMeta, StaticConfigCompiler, SupervisorState, WorkerdSupervisor,
    WorkerdSupervisorOptions, verify_runtime_binary,
};
use open_compute_service::asset_backend::AssetBindingService;
use open_compute_service::runtime_bridge::{
    WorkerdTransport, bind_runtime_source, serve_runtime_source,
};
use open_compute_service::runtime_generation::RuntimeGenerationResources;
use open_compute_service::service_invocations::ServiceInvocationRegistry;
use open_compute_service::{
    SqliteKvBindingExecutor, bind_binding_backend, serve_binding_backend_with_assets,
};
use open_compute_storage::PlatformStorage;
use open_compute_workers::{BundleLimits, DeploymentPins, ResourcePins, RuntimeSource};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub(super) struct Harness {
    pub(super) storage: Arc<PlatformStorage>,
    pub(super) artifacts: ArtifactStore,
    pub(super) transport: WorkerdTransport,
    pub(super) supervisor: Arc<WorkerdSupervisor>,
    #[allow(dead_code)]
    pub(super) deployment_pins: DeploymentPins,
    pub(super) service_invocations: Arc<ServiceInvocationRegistry>,
    shutdown: tokio::sync::watch::Sender<bool>,
    source_task: tokio::task::JoinHandle<Result<(), open_compute_core::PlatformError>>,
    binding_task: tokio::task::JoinHandle<Result<(), open_compute_core::PlatformError>>,
    generation_task: tokio::task::JoinHandle<()>,
    _mock: MockS3,
    _temp: tempfile::TempDir,
}

impl Harness {
    pub(super) async fn start(release: &str) -> Self {
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
        let mock = MockS3::spawn("open-compute").await;
        let artifacts = artifact_store(&mock);
        let cache = Arc::new(
            ArtifactCache::open(
                storage.data_dir().artifact_cache_dir(),
                CacheConfig::default(),
                StartupId::generate(),
            )
            .unwrap(),
        );
        let runtime =
            verify_runtime_binary(&lock, &workerd, Duration::from_secs(10), &Redactor::new())
                .await
                .expect("formal pinned runtime");
        let source_auth = GenerationAuthRegistry::new();
        let binding_auth = GenerationAuthRegistry::new();
        let source_listener = bind_runtime_source().await.unwrap();
        let source_addr = source_listener.local_addr().unwrap();
        let binding_listener = bind_binding_backend().await.unwrap();
        let binding_addr = binding_listener.local_addr().unwrap();
        let deployment_pins = DeploymentPins::new();
        let service_invocations = Arc::new(ServiceInvocationRegistry::new(
            storage.clone(),
            deployment_pins.clone(),
        ));
        let (shutdown, mut source_shutdown) = tokio::sync::watch::channel(false);
        let mut binding_shutdown = shutdown.subscribe();
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
            let pins = deployment_pins.clone();
            let services = service_invocations.clone();
            let asset_service = Arc::new(AssetBindingService::new(
                storage.clone(),
                artifacts.clone(),
                cache,
                pins.clone(),
            ));
            async move {
                serve_binding_backend_with_assets(
                    binding_listener,
                    storage.clone(),
                    auth,
                    ResourcePins::new(),
                    Arc::new(SqliteKvBindingExecutor::new(
                        storage.clone(),
                        Arc::new(SystemClock),
                    )),
                    None,
                    None,
                    None,
                    durable_objects_config(),
                    Default::default(),
                    Default::default(),
                    None,
                    asset_service,
                    services,
                    None,
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
            lock,
            root.join("packages/runtime"),
            storage.data_dir().runtime_dir(),
            PlatformReleaseMeta {
                version: release.to_owned(),
            },
            Duration::from_secs(20),
            Redactor::new(),
        )
        .with_generation_auth(source_auth.clone())
        .with_binding_generation_auth(binding_auth.clone());
        let supervisor_slot = Arc::new(Mutex::new(None));
        let transport = WorkerdTransport::new(source_auth.clone(), supervisor_slot.clone())
            .with_deployment_pins(deployment_pins.clone());
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
                lease_path: Some(
                    storage
                        .data_dir()
                        .runtime_dir()
                        .join(format!("{release}.lease")),
                ),
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

        let generation_task = tokio::spawn({
            let mut lifecycle_shutdown = shutdown.subscribe();
            let mut snapshots = supervisor.subscribe();
            let mut resources = RuntimeGenerationResources::new(
                service_invocations.as_ref().clone(),
                deployment_pins.clone(),
            );
            async move {
                loop {
                    resources.observe(&snapshots.borrow().clone());
                    tokio::select! {
                        changed = snapshots.changed() => {
                            if changed.is_err() { break; }
                        }
                        changed = lifecycle_shutdown.changed() => {
                            if changed.is_err() || *lifecycle_shutdown.borrow() { break; }
                        }
                    }
                }
            }
        });

        Self {
            storage,
            artifacts,
            transport,
            supervisor,
            deployment_pins,
            service_invocations,
            shutdown,
            source_task,
            binding_task,
            generation_task,
            _mock: mock,
            _temp: temp,
        }
    }

    pub(super) async fn stop(self) {
        self.supervisor.shutdown().await;
        assert_eq!(self.supervisor.owner_registry_len(), 0);
        let _ = self.shutdown.send(true);
        self.source_task.await.unwrap().unwrap();
        self.binding_task.await.unwrap().unwrap();
        self.generation_task.await.unwrap();
    }
}

pub(super) async fn wait_running(supervisor: &WorkerdSupervisor, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    let mut rx = supervisor.subscribe();
    loop {
        let snapshot = rx.borrow().clone();
        if snapshot.state == SupervisorState::Running {
            return;
        }
        assert!(
            snapshot.state != SupervisorState::Failed,
            "supervisor failed: {snapshot:?}"
        );
        assert!(Instant::now() < deadline, "supervisor did not become ready");
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

fn durable_objects_config() -> DurableObjectsConfig {
    DurableObjectsConfig {
        disk_high_watermark_percent: 98,
        disk_stop_writes_percent: 99,
        ..DurableObjectsConfig::default()
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
        mock.endpoint,
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

fn repo_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .unwrap()
}
