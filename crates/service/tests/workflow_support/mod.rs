//! Shared real-runtime fixture for Workflow Gates; all scenario data stays here.

use open_compute_artifacts::{
    ArtifactCache, ArtifactStore, MapEnv, MockS3, S3ArtifactClient, resolve_s3_credentials_with,
};
use open_compute_core::clock::SystemClock;
use open_compute_core::config::{PlatformConfig, RuntimeConfig, StorageConfig};
use open_compute_core::{Redactor, RequestId, StartupId};
use open_compute_runtime::{
    DirectoryServicePath, ExternalServiceAddress, GenerationAuthRegistry, OsJitter,
    PlatformReleaseMeta, StaticConfigCompiler, SupervisorState, WorkerdSupervisor,
    WorkerdSupervisorOptions, verify_runtime_binary,
};
use open_compute_service::runtime_bridge::{
    DispatchTarget, WorkerdTransport, bind_runtime_source, serve_runtime_source,
};
use open_compute_storage::{PlatformStorage, WorkerRepository};
use open_compute_workers::{
    BundleLimits, CanonicalBundle, CreateDeploymentOutcome, CreateDeploymentRequest,
    DeploymentController, ModuleInput, ModuleType, RuntimeSource,
};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub(crate) struct Harness {
    pub storage: Arc<PlatformStorage>,
    pub artifacts: ArtifactStore,
    // The hard Gate only exercises cache reads; the product Gate explicitly evicts it.
    #[allow(dead_code)]
    pub cache: Arc<ArtifactCache>,
    pub transport: WorkerdTransport,
    pub supervisor: Arc<WorkerdSupervisor>,
    pub binding_auth: GenerationAuthRegistry,
    pub binding_listener: Option<tokio::net::TcpListener>,
    pub shutdown: tokio::sync::watch::Sender<bool>,
    source_task: Option<tokio::task::JoinHandle<Result<(), open_compute_core::PlatformError>>>,
    // The hard-Gate binary only needs ownership to keep S3 alive; restore/process Gates reuse it.
    #[allow(dead_code)]
    pub(crate) mock: Arc<MockS3>,
    pub(crate) temp: Option<tempfile::TempDir>,
}

impl Harness {
    pub(crate) async fn start() -> Self {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .to_path_buf();
        let runs = root.join(".temp/workflow-run");
        std::fs::create_dir_all(&runs).unwrap();
        let temp = tempfile::Builder::new()
            .prefix("workflow-")
            .tempdir_in(runs)
            .unwrap();
        let data = temp.path().join("data");
        let storage = Arc::new(
            PlatformStorage::bootstrap(
                &StorageConfig {
                    data_dir: data.clone(),
                    master_key_file: data.join("keys/master.key"),
                    master_key_env: None,
                    sqlite_busy_timeout_ms: 5000,
                    free_space_soft_bytes: 1_073_741_824,
                    free_space_hard_bytes: 268_435_456,
                },
                &SystemClock,
            )
            .unwrap(),
        );
        let mock = MockS3::spawn("open-compute").await;
        let config = PlatformConfig::from_toml_str(&format!(
            r#"
[s3]
endpoint = "{}"
region = "us-east-1"
bucket = "open-compute"
force_path_style = true
access_key_id_env = "ACCESS_KEY"
secret_access_key_env = "SECRET_KEY"
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
        let environment = MapEnv::new()
            .with("ACCESS_KEY", "AKIAEXAMPLEKEYID01")
            .with("SECRET_KEY", "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY");
        let credentials = resolve_s3_credentials_with(&config, &environment).unwrap();
        let artifacts = ArtifactStore::new(
            S3ArtifactClient::connect(&config, &credentials, 32 * 1024 * 1024).unwrap(),
        );
        Self::boot(storage, artifacts, Arc::new(mock), temp).await
    }

    pub(crate) async fn boot(
        storage: Arc<PlatformStorage>,
        artifacts: ArtifactStore,
        mock: Arc<MockS3>,
        temp: tempfile::TempDir,
    ) -> Self {
        let workerd = PathBuf::from(
            std::env::var_os("OPEN_COMPUTE_TEST_WORKERD")
                .expect("Workflow Gate requires an already installed, verified workerd"),
        );
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .to_path_buf();
        let lock = root.join("packages/runtime/workerd.lock.json");
        let runtime =
            verify_runtime_binary(&lock, &workerd, Duration::from_secs(10), &Redactor::new())
                .await
                .unwrap();
        let source_auth = GenerationAuthRegistry::new();
        let binding_auth = GenerationAuthRegistry::new();
        let source_listener = bind_runtime_source().await.unwrap();
        let source_addr = source_listener.local_addr().unwrap();
        let binding_listener = bind_runtime_source().await.unwrap();
        let binding_addr = binding_listener.local_addr().unwrap();
        let (shutdown, mut receiver) = tokio::sync::watch::channel(false);
        let cache = Arc::new(
            ArtifactCache::open(
                storage.data_dir().artifact_cache_dir(),
                PlatformConfig::from_toml_str(
                    "[cache]\nmax_bytes=1048576\nmax_artifact_bytes=1048576\n\
                     high_watermark_ratio=0.0001\nlow_watermark_ratio=0.000001\n",
                )
                .unwrap()
                .cache,
                StartupId::generate(),
            )
            .unwrap(),
        );
        let source_task = tokio::spawn({
            let source =
                RuntimeSource::new(storage.clone(), artifacts.clone(), BundleLimits::default())
                    .with_cache(cache.clone());
            let auth = source_auth.clone();
            async move {
                serve_runtime_source(source_listener, source, auth, async move {
                    let _ = receiver.changed().await;
                })
                .await
            }
        });
        let compiler = StaticConfigCompiler::new(
            runtime.clone(),
            lock.clone(),
            root.join("packages/runtime"),
            storage.data_dir().runtime_dir(),
            PlatformReleaseMeta {
                version: "workflow-gate".into(),
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
        let supervisor = Arc::new(WorkerdSupervisor::new(
            WorkerdSupervisorOptions {
                runtime,
                compiler,
                config: RuntimeConfig {
                    startup_timeout_ms: 20000,
                    shutdown_grace_ms: 500,
                    drain_timeout_ms: 100,
                    kill_timeout_ms: 500,
                    restart_budget: 5,
                    restart_window_ms: 60000,
                    restart_backoff_initial_ms: 10,
                    restart_backoff_max_ms: 100,
                },
                clock: Arc::new(SystemClock),
                jitter: Arc::new(OsJitter),
                redactor: Redactor::new(),
                lease_path: Some(storage.data_dir().runtime_dir().join("workflow.lease")),
            },
            vec![
                ExternalServiceAddress::loopback("runtime-source", source_addr).unwrap(),
                ExternalServiceAddress::loopback("binding-backend", binding_addr).unwrap(),
            ],
            vec![DirectoryServicePath::local("do-storage", &do_storage).unwrap()],
            vec![source_auth, binding_auth.clone()],
        ));
        *supervisor_slot.lock().unwrap() = Some(supervisor.clone());
        supervisor.start();
        wait_running(&supervisor, None).await;
        Self {
            storage,
            artifacts,
            cache,
            transport,
            supervisor,
            binding_auth,
            binding_listener: Some(binding_listener),
            shutdown,
            source_task: Some(source_task),
            mock,
            temp: Some(temp),
        }
    }

    pub(crate) async fn deploy(&self, source: &str, class: &str) -> DispatchTarget {
        self.deploy_bound(source, class, BTreeMap::new()).await
    }

    pub(crate) async fn deploy_bound(
        &self,
        source: &str,
        class: &str,
        bindings: BTreeMap<String, open_compute_workers::DeploymentBindingInput>,
    ) -> DispatchTarget {
        let account = self.storage.identity().default_account_id;
        let (worker, _) = WorkerRepository::new(self.storage.db())
            .create_worker(
                account,
                &format!("workflow-{}", RequestId::generate()),
                RequestId::generate(),
                1,
                1_000_000,
            )
            .unwrap();
        self.deploy_worker(worker.id, source, class, bindings).await
    }

    pub(crate) async fn deploy_worker(
        &self,
        worker: open_compute_core::WorkerId,
        source: &str,
        class: &str,
        bindings: BTreeMap<String, open_compute_workers::DeploymentBindingInput>,
    ) -> DispatchTarget {
        let account = self.storage.identity().default_account_id;
        let bundle = CanonicalBundle::build(
            "index.js",
            vec![ModuleInput {
                name: "index.js".into(),
                module_type: ModuleType::EsModule,
                bytes: source.as_bytes().to_vec(),
            }],
            BundleLimits::default(),
        )
        .unwrap();
        let controller = DeploymentController::new(
            &self.storage,
            self.artifacts.clone(),
            Arc::new(self.transport.clone()),
            BundleLimits::default(),
        );
        let result = controller
            .create_deployment(CreateDeploymentRequest {
                account_id: account,
                worker_id: worker,
                idempotency_key: RequestId::generate().to_string(),
                content: open_compute_workers::DeploymentContent::Worker {
                    bundle: bundle.into_bytes().into(),
                    assets: None,
                },
                compatibility_date: "2026-08-22".into(),
                compatibility_flags: vec!["rpc".into()],
                vars: BTreeMap::from([("MODE".into(), serde_json::json!("frozen"))]),
                secrets: BTreeMap::new(),
                bindings,
                services: BTreeMap::new(),
                queue_consumers: Vec::new(),
                crons: None,
                limits: serde_json::json!({"profile":"default"}),
                promote: true,
                request_id: RequestId::generate(),
                now_ms: 1,
            })
            .await
            .unwrap();
        let CreateDeploymentOutcome::Applied(result) = result else {
            panic!("unexpected replay")
        };
        DispatchTarget {
            account_id: account,
            worker_id: worker,
            deployment_id: result.deployment.id,
            worker_code_sha256: hex::encode(result.deployment.worker_code_sha256),
            entrypoint: Some(class.into()),
            route_generation: 1,
            request_id: RequestId::generate(),
        }
    }

    pub(crate) async fn restart(&self) {
        let previous = self.supervisor.snapshot().pid;
        self.supervisor.report_unhealthy();
        wait_running(&self.supervisor, previous).await;
    }

    pub(crate) async fn quiesce(&mut self) {
        self.supervisor.shutdown().await;
        assert_eq!(self.supervisor.owner_registry_len(), 0);
        let _ = self.shutdown.send(true);
        if let Some(task) = self.source_task.take() {
            task.await.unwrap().unwrap();
        }
    }

    pub(crate) async fn stop(mut self) {
        self.quiesce().await;
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        let mut cleanup_failed = false;
        if let Some(source_task) = self.source_task.take() {
            // Keep the multi-thread test runtime alive until its process owner has reaped
            // the child. Moving the data directory first would strand its lease path.
            let runtime = tokio::runtime::Handle::current();
            let supervisor = self.supervisor.clone();
            let shutdown = self.shutdown.clone();
            let cleanup = std::thread::spawn(move || {
                runtime.block_on(async {
                    tokio::time::timeout(Duration::from_secs(10), async move {
                        supervisor.shutdown().await;
                        let _ = shutdown.send(true);
                        source_task.await
                    })
                    .await
                })
            })
            .join();
            if !matches!(cleanup, Ok(Ok(Ok(Ok(()))))) {
                cleanup_failed = true;
                eprintln!("Workflow Gate process cleanup did not complete; lease retained");
            }
        }
        if (std::thread::panicking() || cleanup_failed)
            && let Some(temp) = self.temp.take()
        {
            let path = temp.keep();
            let failed = path.parent().unwrap().join("failed");
            let _ = std::fs::create_dir_all(&failed);
            let _ = std::fs::write(
                path.join("diagnostics.txt"),
                format!("{:?}", self.supervisor.last_diagnostics()),
            );
            let _ = std::fs::rename(&path, failed.join(path.file_name().unwrap()));
        }
        assert!(
            !cleanup_failed || std::thread::panicking(),
            "Workflow Gate child cleanup failed"
        );
    }
}

async fn wait_running(supervisor: &WorkerdSupervisor, previous: Option<i32>) {
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut receiver = supervisor.subscribe();
    loop {
        let snapshot = supervisor.snapshot();
        if snapshot.state == SupervisorState::Running && snapshot.pid != previous {
            return;
        }
        assert!(
            Instant::now() < deadline && snapshot.state != SupervisorState::Failed,
            "runtime unavailable: {snapshot:?} {:?}",
            supervisor.last_diagnostics()
        );
        let _ = tokio::time::timeout(Duration::from_millis(100), receiver.changed()).await;
    }
}
