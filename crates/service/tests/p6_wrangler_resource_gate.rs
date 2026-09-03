//! Fixed Wrangler resource commands against the real local v4 composition.

#![cfg(feature = "test-support")]

#[path = "p6_wrangler_resource_gate/search.rs"]
mod search;

use axum::Router;
use axum::middleware;
use open_compute_artifacts::{
    ArtifactStore, MapEnv, MockS3, R2ObjectStore, S3ArtifactClient, resolve_s3_credentials_with,
};
use open_compute_core::config::{
    D1Config, KvConfig, MetricsConfig, PlatformConfig, R2Config, SchedulerConfig, SecretReference,
    ServerConfig, StorageConfig,
};
use open_compute_core::{RequestId, SystemClock, SystemSchedulerClock, VersionId};
use open_compute_runtime::{GenerationAuthRegistry, WorkerdSupervisor};
use open_compute_service::http::{HttpState, merged_router};
use open_compute_service::runtime_bridge::WorkerdTransport;
use open_compute_service::workflow_http::WorkflowApiState;
use open_compute_service::{
    D1ApiState, D1BindingService, HealthCoordinator, KvApiState, MetricsRegistry, QueueApiState,
    R2ApiState, R2BindingService, SchedulerService, SqliteKvBindingExecutor,
};
use open_compute_storage::{
    NewVersion, NewVersionProducts, PlatformStorage, SchedulerStore, VersionContentKind,
    WorkerRepository, WorkflowRepository,
};
use open_compute_workers::ResourcePins;
use serde_json::Value;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::Output;
use std::sync::{Arc, Mutex};
use std::time::Duration;

const WRANGLER_VERSION: &str = "4.127.1";
const ADMIN_TOKEN: &str = "p6-wrangler-resource-gate-admin-token";
const TOKEN: &str = "p6-wrangler-resource-gate-deployer-token";
const READ_ONLY_TOKEN: &str = "p6-wrangler-resource-gate-read-only-token";
const KV_NAME: &str = "resource-gate-kv";
const D1_NAME: &str = "resource-gate-d1";
const R2_NAME: &str = "resource-gate-r2";
const QUEUE_NAME: &str = "resource-gate-queue";
const WORKFLOW_NAME: &str = "resource-gate-workflow";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fixed_wrangler_resource_commands_use_live_v4_authorities() {
    let fixture = Fixture::new().await;
    let requests = Arc::new(Mutex::new(Vec::new()));
    let traced = requests.clone();
    let app = fixture.app.layer(middleware::from_fn(
        move |request: axum::extract::Request, next: middleware::Next| {
            let traced = traced.clone();
            async move {
                traced
                    .lock()
                    .unwrap()
                    .push(format!("{} {}", request.method(), request.uri()));
                next.run(request).await
            }
        },
    ));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let command = WranglerCommand {
        executable: fixed_wrangler(),
        project: fixture.project.path(),
        api_base_url: format!("{origin}/client/v4"),
        account_id: &fixture.public_account,
    };

    assert_success(&command.run(&["--version"]).await);
    exercise_kv(&command, fixture.project.path()).await;
    exercise_d1(&command, fixture.project.path()).await;
    exercise_r2(&command, fixture.project.path()).await;
    exercise_queues(&command).await;
    exercise_workflows(&command).await;
    search::exercise_vectorize(&command, fixture.project.path(), &fixture.search).await;
    search::exercise_ai_search(&command).await;

    server.abort();
    let trace = requests.lock().unwrap();
    for fragment in [
        "/storage/kv/namespaces",
        "/d1/database",
        "/r2/buckets",
        "/queues",
        "/workflows",
        "/vectorize/v2/indexes",
        "/ai-search/namespaces",
        "/ai-search/tokens",
    ] {
        assert!(
            trace.iter().any(|line| line.contains(fragment)),
            "fixed Wrangler did not reach {fragment}: {trace:?}"
        );
    }
    assert!(
        trace.iter().all(|line| line.contains(" /client/v4/")),
        "resource Gate escaped the local v4 API: {trace:?}"
    );
}

async fn exercise_kv(command: &WranglerCommand<'_>, project: &Path) {
    assert_success(
        &command
            .run(&[
                "kv",
                "namespace",
                "create",
                KV_NAME,
                "--config",
                "wrangler.jsonc",
            ])
            .await,
    );
    let listed = command
        .run(&["kv", "namespace", "list", "--config", "wrangler.jsonc"])
        .await;
    assert_success(&listed);
    let namespaces = json_stdout(&listed);
    let namespace_id = namespaces
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["title"] == KV_NAME)
        .and_then(|item| item["id"].as_str())
        .unwrap()
        .to_owned();

    write_config(project, command.account_id, Some(&namespace_id), None);
    for args in [
        vec![
            "kv",
            "key",
            "put",
            "greeting",
            "你好 🌍",
            "--namespace-id",
            &namespace_id,
            "--remote",
            "--config",
            "wrangler.jsonc",
        ],
        vec![
            "kv",
            "key",
            "list",
            "--namespace-id",
            &namespace_id,
            "--remote",
            "--config",
            "wrangler.jsonc",
        ],
    ] {
        assert_success(&command.run(&args).await);
    }
    let value = command
        .run(&[
            "kv",
            "key",
            "get",
            "greeting",
            "--namespace-id",
            &namespace_id,
            "--remote",
            "--text",
            "--config",
            "wrangler.jsonc",
        ])
        .await;
    assert_success(&value);
    assert_eq!(String::from_utf8_lossy(&value.stdout).trim(), "你好 🌍");
    assert_success(
        &command
            .run(&[
                "kv",
                "key",
                "delete",
                "greeting",
                "--namespace-id",
                &namespace_id,
                "--remote",
                "--config",
                "wrangler.jsonc",
            ])
            .await,
    );
    assert_success(
        &command
            .run(&[
                "kv",
                "namespace",
                "delete",
                "--namespace-id",
                &namespace_id,
                "--skip-confirmation",
                "--config",
                "wrangler.jsonc",
            ])
            .await,
    );
}

async fn exercise_d1(command: &WranglerCommand<'_>, project: &Path) {
    assert_success(
        &command
            .run(&["d1", "create", D1_NAME, "--config", "wrangler.jsonc"])
            .await,
    );
    let listed = command
        .run(&["d1", "list", "--json", "--config", "wrangler.jsonc"])
        .await;
    assert_success(&listed);
    let databases = json_stdout(&listed);
    let database_id = databases
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["name"] == D1_NAME)
        .and_then(|item| item["uuid"].as_str())
        .unwrap()
        .to_owned();
    write_config(project, command.account_id, None, Some(&database_id));
    std::fs::create_dir(project.join("migrations")).unwrap();
    std::fs::write(
        project.join("migrations/0001_create_items.sql"),
        "CREATE TABLE items (id INTEGER PRIMARY KEY, value TEXT NOT NULL);",
    )
    .unwrap();

    for args in [
        vec![
            "d1",
            "info",
            D1_NAME,
            "--json",
            "--config",
            "wrangler.jsonc",
        ],
        vec![
            "d1",
            "execute",
            D1_NAME,
            "--remote",
            "--command",
            "SELECT 42 AS answer",
            "--json",
            "--config",
            "wrangler.jsonc",
        ],
        vec![
            "d1",
            "migrations",
            "apply",
            D1_NAME,
            "--remote",
            "--config",
            "wrangler.jsonc",
        ],
    ] {
        assert_success(&command.run(&args).await);
    }
    assert_success(
        &command
            .run(&[
                "d1",
                "delete",
                D1_NAME,
                "--skip-confirmation",
                "--config",
                "wrangler.jsonc",
            ])
            .await,
    );
}

async fn exercise_r2(command: &WranglerCommand<'_>, project: &Path) {
    assert_success(
        &command
            .run(&[
                "r2",
                "bucket",
                "create",
                R2_NAME,
                "--config",
                "wrangler.jsonc",
            ])
            .await,
    );
    let listed = command
        .run(&["r2", "bucket", "list", "--config", "wrangler.jsonc"])
        .await;
    assert_success(&listed);
    assert!(String::from_utf8_lossy(&listed.stdout).contains(R2_NAME));
    std::fs::write(project.join("r2-input.bin"), b"fixed-wrangler-r2\0payload").unwrap();
    assert_success(
        &command
            .run(&[
                "r2",
                "object",
                "put",
                &format!("{R2_NAME}/folder/object.bin"),
                "--file",
                "r2-input.bin",
                "--remote",
                "--config",
                "wrangler.jsonc",
            ])
            .await,
    );
    assert_success(
        &command
            .run(&[
                "r2",
                "object",
                "get",
                &format!("{R2_NAME}/folder/object.bin"),
                "--file",
                "r2-output.bin",
                "--remote",
                "--config",
                "wrangler.jsonc",
            ])
            .await,
    );
    assert_eq!(
        std::fs::read(project.join("r2-output.bin")).unwrap(),
        b"fixed-wrangler-r2\0payload"
    );
    assert_success(
        &command
            .run(&[
                "r2",
                "object",
                "delete",
                &format!("{R2_NAME}/folder/object.bin"),
                "--remote",
                "--force",
                "--config",
                "wrangler.jsonc",
            ])
            .await,
    );
    assert_success(
        &command
            .run(&[
                "r2",
                "bucket",
                "delete",
                R2_NAME,
                "--config",
                "wrangler.jsonc",
            ])
            .await,
    );
}

async fn exercise_queues(command: &WranglerCommand<'_>) {
    assert_success(
        &command
            .run(&["queues", "create", QUEUE_NAME, "--config", "wrangler.jsonc"])
            .await,
    );
    let listed = command
        .run(&["queues", "list", "--config", "wrangler.jsonc"])
        .await;
    assert_success(&listed);
    assert!(String::from_utf8_lossy(&listed.stdout).contains(QUEUE_NAME));
    assert_success(
        &command
            .run(&["queues", "delete", QUEUE_NAME, "--config", "wrangler.jsonc"])
            .await,
    );
}

async fn exercise_workflows(command: &WranglerCommand<'_>) {
    let listed = command
        .run(&["workflows", "list", "--config", "wrangler.jsonc"])
        .await;
    assert_success(&listed);
    assert!(String::from_utf8_lossy(&listed.stdout).contains(WORKFLOW_NAME));
    assert_success(
        &command
            .run(&[
                "workflows",
                "describe",
                WORKFLOW_NAME,
                "--config",
                "wrangler.jsonc",
            ])
            .await,
    );
    assert_success(
        &command
            .run(&[
                "workflows",
                "delete",
                WORKFLOW_NAME,
                "--config",
                "wrangler.jsonc",
            ])
            .await,
    );
}

struct Fixture {
    _temp: tempfile::TempDir,
    _mock: MockS3,
    project: tempfile::TempDir,
    app: Router,
    public_account: String,
    search: search::SearchFixture,
}

impl Fixture {
    async fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("data");
        let storage =
            Arc::new(PlatformStorage::bootstrap(&storage_config(&root), &SystemClock).unwrap());
        let mock = MockS3::spawn("open-compute").await;
        let (artifacts, objects, s3) = stores(&mock);
        let pins = ResourcePins::new();
        let search = search::SearchFixture::new(storage.clone(), pins.clone(), s3).await;
        let metrics = Arc::new(
            MetricsRegistry::new(&MetricsConfig::default(), "p6-wrangler", "local-v4").unwrap(),
        );
        let transport = WorkerdTransport::new(
            GenerationAuthRegistry::new(),
            Arc::new(Mutex::<Option<Arc<WorkerdSupervisor>>>::new(None)),
        );
        let scheduler_store = Arc::new(
            SchedulerStore::open(&storage.data_dir().ensure_scheduler_db().unwrap(), 5_000, 1)
                .unwrap(),
        );
        let scheduler = Arc::new(SchedulerService::new(
            scheduler_store.clone(),
            storage.clone(),
            transport.clone(),
            SchedulerConfig::default(),
            Default::default(),
            Arc::new(SystemSchedulerClock),
        ));
        seed_workflow(&storage);
        let r2_config = R2Config::default();
        let r2_binding = Arc::new(
            R2BindingService::new(
                storage.clone(),
                pins.clone(),
                objects.clone(),
                r2_config.clone(),
            )
            .unwrap(),
        );
        let d1_config = D1Config::default();
        let state = HttpState::new(
            HealthCoordinator::new(),
            metrics.clone(),
            false,
            false,
            &server_config(temp.path()),
        )
        .unwrap()
        .with_kv_api(KvApiState::new(
            storage.clone(),
            artifacts.clone(),
            pins.clone(),
            Arc::new(SqliteKvBindingExecutor::new(
                storage.clone(),
                Arc::new(SystemClock),
            )),
            KvConfig::default(),
            1_000,
            Duration::from_secs(1),
        ))
        .with_d1_api(D1ApiState::new(
            storage.clone(),
            artifacts,
            pins.clone(),
            Arc::new(D1BindingService::new(
                storage.clone(),
                pins.clone(),
                d1_config.clone(),
            )),
            d1_config,
            1_000,
            Duration::from_secs(1),
        ))
        .with_r2_api(
            R2ApiState::new(
                storage.clone(),
                objects,
                pins,
                r2_config,
                Duration::from_secs(1),
            )
            .with_binding(r2_binding),
        )
        .with_queue_api(Some(QueueApiState::new(
            storage.clone(),
            scheduler.clone(),
            32,
        )))
        .with_workflow_api(Some(WorkflowApiState::new(
            storage.clone(),
            scheduler_store,
            transport,
            Default::default(),
        )))
        .with_scheduler(Some(scheduler))
        .with_search_api(search.api.clone());
        let (state, public_account) = open_compute_service::cloudflare_v4_for_test(state, storage);
        let project = tempfile::tempdir().unwrap();
        std::fs::create_dir(project.path().join("xdg")).unwrap();
        std::fs::write(
            project.path().join("index.ts"),
            "export default { fetch() {} };",
        )
        .unwrap();
        write_config(project.path(), &public_account, None, None);
        Self {
            _temp: temp,
            _mock: mock,
            project,
            app: merged_router(state),
            public_account,
            search,
        }
    }
}

fn seed_workflow(storage: &PlatformStorage) {
    let account = storage.identity().default_account_id;
    let workers = WorkerRepository::new(storage.db());
    let (worker, _) = workers
        .create_worker(
            account,
            "resource-gate-worker",
            RequestId::generate(),
            1,
            1_000_000,
        )
        .unwrap();
    let worker_version = VersionId::generate();
    workers
        .insert_staging_version(
            &NewVersion {
                id: worker_version,
                account_id: account,
                worker_id: worker.id,
                content_kind: VersionContentKind::Worker,
                artifact_sha256: Some([1; 32]),
                artifact_size: Some(1),
                artifact_schema_version: Some(1),
                main_module: Some("index.js".into()),
                worker_code_sha256: [2; 32],
                compatibility_date: "2026-08-30".into(),
                compatibility_flags: Vec::new(),
                vars: Default::default(),
                secrets: Default::default(),
                request_id: RequestId::generate(),
                now_ms: 2,
            },
            &NewVersionProducts::default(),
            1_000_000,
        )
        .unwrap();
    workers.begin_validation(worker_version).unwrap();
    workers.mark_ready(worker_version, 3).unwrap();
    workers
        .promote(
            account,
            worker.id,
            worker_version,
            None,
            RequestId::generate(),
            4,
        )
        .unwrap();
    let workflows = WorkflowRepository::new(storage.db());
    let definition = workflows
        .create_definition(account, WORKFLOW_NAME, 5)
        .unwrap();
    let version = workflows
        .stage_version(account, definition.id, worker_version, "ResourceFlow", 6)
        .unwrap();
    workflows
        .finish_version(account, version.target.workflow_version_id, true, 7)
        .unwrap();
}

fn write_config(project: &Path, account_id: &str, kv_id: Option<&str>, d1_id: Option<&str>) {
    let schema = fixed_wrangler()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("config-schema.json");
    let mut config = serde_json::json!({
        "$schema": schema,
        "name": "p6-wrangler-resource-gate",
        "main": "index.ts",
        "account_id": account_id,
        "compatibility_date": "2026-08-30",
        "workers_dev": false,
        "send_metrics": false,
    });
    if let Some(id) = kv_id {
        config["kv_namespaces"] = serde_json::json!([{"binding":"KV", "id":id}]);
    }
    if let Some(id) = d1_id {
        config["d1_databases"] = serde_json::json!([{
            "binding":"DB", "database_name":D1_NAME, "database_id":id,
            "migrations_dir":"migrations"
        }]);
    }
    std::fs::write(
        project.join("wrangler.jsonc"),
        serde_json::to_vec_pretty(&config).unwrap(),
    )
    .unwrap();
}

struct WranglerCommand<'a> {
    executable: PathBuf,
    project: &'a Path,
    api_base_url: String,
    account_id: &'a str,
}

impl WranglerCommand<'_> {
    async fn run(&self, args: &[&str]) -> Output {
        assert!(self.api_base_url.starts_with("http://127.0.0.1:"));
        let mut command = tokio::process::Command::new(&self.executable);
        command
            .args(args)
            .current_dir(self.project)
            .env("CLOUDFLARE_API_BASE_URL", &self.api_base_url)
            .env("CLOUDFLARE_API_TOKEN", TOKEN)
            .env("CLOUDFLARE_ACCOUNT_ID", self.account_id)
            .env("WRANGLER_SEND_METRICS", "false")
            .env("WRANGLER_SEND_ERROR_REPORTS", "false")
            .env("WRANGLER_NO_SKILLS_UPDATE_PROMPTS", "true")
            .env("WRANGLER_HIDE_BANNER", "true")
            .env("DO_NOT_TRACK", "1")
            .env("CI", "true")
            .env("XDG_CONFIG_HOME", self.project.join("xdg"))
            .env("HTTP_PROXY", "http://127.0.0.1:9")
            .env("HTTPS_PROXY", "http://127.0.0.1:9")
            .env("NO_PROXY", "127.0.0.1,localhost")
            .env_remove("CF_API_BASE_URL")
            .env_remove("CLOUDFLARE_BASE_URL")
            .env_remove("CLOUDFLARE_API_KEY")
            .env_remove("CLOUDFLARE_EMAIL")
            .kill_on_drop(true);
        tokio::time::timeout(Duration::from_secs(60), command.output())
            .await
            .expect("fixed Wrangler resource command timed out")
            .expect("fixed Wrangler and Node.js must already be installed")
    }
}

fn fixed_wrangler() -> PathBuf {
    let root = repo_root();
    let lock = std::fs::read_to_string(root.join("bun.lock")).unwrap();
    assert!(lock.contains("\"wrangler\": [\"wrangler@4.127.1\""));
    let prefix = format!("wrangler@{WRANGLER_VERSION}+");
    let mut installs = std::fs::read_dir(root.join("node_modules/.bun"))
        .expect("locked Bun dependencies must already be installed")
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name().to_string_lossy().starts_with(&prefix))
        .map(|entry| entry.path())
        .collect::<Vec<_>>();
    installs.sort();
    assert_eq!(
        installs.len(),
        1,
        "exactly one fixed Wrangler must be installed"
    );
    let package = installs[0].join("node_modules/wrangler");
    let metadata: Value =
        serde_json::from_slice(&std::fs::read(package.join("package.json")).unwrap()).unwrap();
    assert_eq!(metadata["version"], WRANGLER_VERSION);
    package.join("bin/wrangler.js")
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    for bytes in [&output.stdout, &output.stderr] {
        let text = String::from_utf8_lossy(bytes);
        for secret in [ADMIN_TOKEN, TOKEN, READ_ONLY_TOKEN] {
            assert!(!text.contains(secret));
        }
        assert!(!text.contains("api.cloudflare.com"));
    }
}

fn json_stdout(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "Wrangler JSON output was invalid: {error}: {}",
            String::from_utf8_lossy(&output.stdout)
        )
    })
}

fn stores(mock: &MockS3) -> (ArtifactStore, R2ObjectStore, S3ArtifactClient) {
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
    let client = S3ArtifactClient::connect(&config, &credentials, 32 * 1024 * 1024).unwrap();
    (
        ArtifactStore::new(client.clone()),
        R2ObjectStore::new(client.clone()),
        client,
    )
}

fn storage_config(root: &Path) -> StorageConfig {
    StorageConfig {
        data_dir: root.to_owned(),
        master_key_file: root.join("keys/master.key"),
        master_key_env: None,
        sqlite_busy_timeout_ms: 5_000,
        free_space_soft_bytes: 1_073_741_824,
        free_space_hard_bytes: 1,
    }
}

fn server_config(root: &Path) -> ServerConfig {
    ServerConfig {
        admin_auth: token(root.join("admin.token"), ADMIN_TOKEN),
        deployer_auth: token(root.join("deployer.token"), TOKEN),
        read_only_auth: token(root.join("read-only.token"), READ_ONLY_TOKEN),
        ..ServerConfig::default()
    }
}

fn token(path: PathBuf, value: &str) -> SecretReference {
    std::fs::write(&path, format!("{value}\n")).unwrap();
    let mut permissions = std::fs::metadata(&path).unwrap().permissions();
    permissions.set_mode(0o600);
    std::fs::set_permissions(&path, permissions).unwrap();
    SecretReference {
        env: None,
        file: Some(path),
    }
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_owned()
}
