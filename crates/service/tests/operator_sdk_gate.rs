//! Operator SDK contract against the live admin router HTTP stack.

use axum::Router;
use open_compute_artifacts::{
    ArtifactStore, MapEnv, MockS3, R2ObjectStore, S3ArtifactClient, resolve_s3_credentials_with,
};
use open_compute_core::config::{MetricsConfig, SecretReference, ServerConfig, StorageConfig};
use open_compute_core::{D1Config, KvConfig, PlatformError, R2Config, WorkflowsConfig};
use open_compute_runtime::GenerationAuthRegistry;
use open_compute_service::SqliteKvBindingExecutor;
use open_compute_service::d1_backend::D1BindingService;
use open_compute_service::d1_http::D1ApiState;
use open_compute_service::health::HealthCoordinator;
use open_compute_service::http::{HttpState, admin_router};
use open_compute_service::kv_http::KvApiState;
use open_compute_service::metrics::MetricsRegistry;
use open_compute_service::queue_http::QueueApiState;
use open_compute_service::r2_backend::R2BindingService;
use open_compute_service::r2_http::R2ApiState;
use open_compute_service::runtime_bridge::WorkerdTransport;
use open_compute_service::workers_http::WorkerApiState;
use open_compute_service::workflow_http::WorkflowApiState;
use open_compute_storage::{PlatformStorage, SchedulerStore};
use open_compute_workers::{BundleLimits, ResourcePins, VersionPins};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tempfile::TempDir;
use tokio::net::TcpListener;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn operator_sdk_matches_live_admin_router_contract() {
    let root = repo_root();
    let build = Command::new("bun")
        .args(["run", "--filter", "@open-compute/operator-sdk", "build"])
        .current_dir(&root)
        .status()
        .expect("run operator-sdk build");
    assert!(build.success(), "operator-sdk build failed");

    let temp = TempDir::new().expect("tempdir");
    let data_dir = temp.path().join("data");
    let storage = Arc::new(
        PlatformStorage::bootstrap(
            &StorageConfig {
                data_dir: data_dir.clone(),
                master_key_file: data_dir.join("keys/master.key"),
                master_key_env: None,
                sqlite_busy_timeout_ms: 5_000,
                free_space_soft_bytes: 1_073_741_824,
                free_space_hard_bytes: 268_435_456,
            },
            &open_compute_core::SystemClock,
        )
        .expect("platform storage"),
    );
    let mock = MockS3::spawn("open-compute").await;
    let client = artifact_client(&mock);
    let artifacts = ArtifactStore::new(client.clone());
    let worker_api = WorkerApiState::new(
        storage.clone(),
        artifacts.clone(),
        WorkerdTransport::new(GenerationAuthRegistry::new(), Arc::new(Mutex::new(None))),
        VersionPins::new(),
        BundleLimits::default(),
        Duration::from_secs(1),
    );

    let admin_token = write_admin_secret(&temp.path().join("admin.token"));
    let server = ServerConfig {
        admin_auth: SecretReference {
            env: None,
            file: Some(admin_token),
        },
        ..ServerConfig::default()
    };
    let state = build_operator_http_state(storage, artifacts, client, worker_api, &server)
        .await
        .expect("operator contract router state");
    let app: Router = admin_router(state);
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind ephemeral admin listener");
    let addr = listener.local_addr().expect("admin listener address");
    let server = tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("serve admin router");
    });

    let status = Command::new("bun")
        .args(["test", "packages/operator-sdk/tests/contract-ocd.test.mjs"])
        .current_dir(&root)
        .env(
            "OPEN_COMPUTE_OPERATOR_BASE_URL",
            format!("http://{addr}/operator/api/v1/"),
        )
        .env("OPEN_COMPUTE_ADMIN_TOKEN", "admin-secret")
        .env("OPEN_COMPUTE_OPERATOR_CONTRACT_SCOPE", "router")
        .status()
        .expect("run operator-sdk contract test");
    server.abort();
    assert!(status.success(), "operator-sdk contract test failed");
}

async fn build_operator_http_state(
    storage: Arc<PlatformStorage>,
    artifacts: ArtifactStore,
    client: S3ArtifactClient,
    worker_api: WorkerApiState,
    server: &ServerConfig,
) -> Result<HttpState, PlatformError> {
    let health = HealthCoordinator::new();
    let metrics = Arc::new(MetricsRegistry::new(
        &MetricsConfig::default(),
        "test",
        "workerd",
    )?);
    let scheduler = Arc::new(SchedulerStore::open(
        &storage.data_dir().ensure_scheduler_db()?,
        100,
        1,
    )?);
    let transport =
        WorkerdTransport::new(GenerationAuthRegistry::new(), Arc::new(Mutex::new(None)));
    let resource_pins = ResourcePins::new();
    let delete_drain = Duration::from_millis(1_000);
    let max_resources = 256_u32;
    let r2_objects = R2ObjectStore::new(client.clone());
    let binding_executor = Arc::new(SqliteKvBindingExecutor::with_config(
        storage.clone(),
        Arc::new(open_compute_core::SystemClock),
        &KvConfig::default(),
    ));
    let r2_backend = Arc::new(
        R2BindingService::new(
            storage.clone(),
            resource_pins.clone(),
            r2_objects.clone(),
            R2Config::default(),
        )?
        .with_metrics(metrics.clone()),
    );
    let r2_api = R2ApiState::new(
        storage.clone(),
        r2_objects,
        resource_pins.clone(),
        R2Config::default(),
        delete_drain,
    )
    .with_metrics(metrics.clone())
    .with_binding(r2_backend);
    r2_api.reconcile_pending().await?;
    let d1_backend = Arc::new(
        D1BindingService::new(storage.clone(), resource_pins.clone(), D1Config::default())
            .with_metrics(metrics.clone()),
    );
    let queue_api = QueueApiState::new(storage.clone(), scheduler.clone());
    queue_api.reconcile_pending().await?;
    Ok(
        HttpState::new(health, metrics.clone(), false, false, server)?
            .with_worker_api(worker_api)
            .with_kv_api(KvApiState::new(
                storage.clone(),
                artifacts.clone(),
                resource_pins.clone(),
                binding_executor,
                KvConfig::default(),
                max_resources,
                delete_drain,
            ))
            .with_r2_api(r2_api)
            .with_d1_api(D1ApiState::new(
                storage.clone(),
                artifacts,
                resource_pins.clone(),
                d1_backend,
                D1Config::default(),
                max_resources,
                delete_drain,
            ))
            .with_queue_api(Some(queue_api))
            .with_workflow_api(Some(WorkflowApiState::new(
                storage,
                scheduler,
                transport,
                WorkflowsConfig::default(),
            ))),
    )
}

fn artifact_client(mock: &MockS3) -> S3ArtifactClient {
    let config = s3_config(mock);
    let env = MapEnv::new()
        .with("S3_ACCESS_KEY_ID", "AKIAEXAMPLEKEYID01")
        .with(
            "S3_SECRET_ACCESS_KEY",
            "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY",
        );
    let credentials = resolve_s3_credentials_with(&config, &env).unwrap();
    S3ArtifactClient::connect(&config, &credentials, 32 * 1024 * 1024).unwrap()
}

fn s3_config(mock: &MockS3) -> open_compute_core::S3Config {
    open_compute_core::PlatformConfig::from_toml_str(&format!(
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
    .s3
}

fn write_admin_secret(path: &Path) -> PathBuf {
    fs::write(path, "admin-secret\n").expect("write admin token");
    let mut permissions = fs::metadata(path)
        .expect("admin token metadata")
        .permissions();
    permissions.set_mode(0o600);
    fs::set_permissions(path, permissions).expect("admin token permissions");
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
