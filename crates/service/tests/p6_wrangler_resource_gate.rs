//! Fixed Wrangler resource commands against the real local v4 composition.

#![cfg(feature = "test-support")]

#[path = "p6_wrangler_resource_gate/evidence.rs"]
mod evidence;
#[path = "p6_wrangler_resource_gate/search.rs"]
mod search;

#[allow(
    dead_code,
    reason = "this Gate reuses the production-process ownership half of the Workflow fixture"
)]
#[path = "workflow_support/platform_process.rs"]
mod platform_process;

use axum::body::{Body, to_bytes};
use axum::http::Request;
use evidence::Evidence;
use open_compute_artifacts::MockS3;
use open_compute_core::config::StorageConfig;
use open_compute_core::{Redactor, RequestId, SystemClock, VersionId};
use open_compute_runtime::verify_runtime_binary;
use open_compute_storage::{
    NewVersion, NewVersionProducts, PlatformStorage, VersionContentKind, WorkerRepository,
    WorkflowRepository,
};
use serde_json::Value;
use std::fs;
use std::io::Write as _;
use std::net::SocketAddr;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::Output;
use std::time::{Duration, Instant};

const WRANGLER_VERSION: &str = "4.127.1";
const ADMIN_TOKEN: &str = platform_process::ADMIN_TOKEN;
const TOKEN: &str = "p6-wrangler-resource-gate-deployer-token";
const READ_ONLY_TOKEN: &str = "p6-wrangler-resource-gate-read-only-token";
const S3_ACCESS_KEY: &str = "AKIAEXAMPLEKEYID01";
const S3_SECRET_KEY: &str = "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY";
const KV_NAME: &str = "resource-gate-kv";
const D1_NAME: &str = "resource-gate-d1";
const R2_NAME: &str = "resource-gate-r2";
const QUEUE_NAME: &str = "resource-gate-queue";
const WORKFLOW_NAME: &str = "resource-gate-workflow";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fixed_wrangler_resource_commands_use_live_v4_authorities() {
    let mut fixture = Fixture::new().await;
    let command = WranglerCommand {
        executable: fixed_wrangler(),
        project: &fixture.project,
        api_base_url: format!("http://{}/client/v4", fixture.admin_addr),
        account_id: &fixture.public_account,
    };

    assert_success(&command.run(&["--version"]).await);
    exercise_kv(&command, &fixture.project).await;
    exercise_d1(&command, &fixture.project).await;
    exercise_r2(&command, &fixture.project).await;
    exercise_queues(&command).await;
    exercise_workflows(&command).await;
    search::exercise_vectorize(&command, &fixture.project).await;
    search::exercise_ai_search(&command).await;

    let log = fs::read(&fixture.log).unwrap_or_default();
    assert_clean_output(&log);
    assert!(
        fixture.process.0.try_wait().unwrap().is_none(),
        "ocd exited while fixed Wrangler was using the admin v4 listener: {}",
        String::from_utf8_lossy(&log),
    );
    fixture.process.stop().await;
    assert!(
        tokio::net::TcpStream::connect(fixture.public_addr)
            .await
            .is_err(),
        "normal shutdown left the public listener reachable",
    );
    assert!(
        tokio::net::TcpStream::connect(fixture.admin_addr)
            .await
            .is_err(),
        "normal shutdown left the admin listener reachable",
    );
    assert_clean_output(&fs::read(&fixture.log).unwrap_or_default());
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
    fs::create_dir(project.join("migrations")).unwrap();
    fs::write(
        project.join("migrations/0001_create_items.sql"),
        "CREATE TABLE items (id INTEGER PRIMARY KEY, value TEXT NOT NULL);",
    )
    .unwrap();

    let info = command
        .run(&[
            "d1",
            "info",
            D1_NAME,
            "--json",
            "--config",
            "wrangler.jsonc",
        ])
        .await;
    assert_success(&info);
    assert!(json_contains(
        &json_stdout(&info),
        "name",
        &Value::from(D1_NAME)
    ));

    let answer = command
        .run(&[
            "d1",
            "execute",
            D1_NAME,
            "--remote",
            "--command",
            "SELECT 42 AS answer",
            "--json",
            "--config",
            "wrangler.jsonc",
        ])
        .await;
    assert_success(&answer);
    assert!(json_contains(
        &json_stdout(&answer),
        "answer",
        &Value::from(42)
    ));

    assert_success(
        &command
            .run(&[
                "d1",
                "migrations",
                "apply",
                D1_NAME,
                "--remote",
                "--config",
                "wrangler.jsonc",
            ])
            .await,
    );
    let migrated = command
        .run(&[
            "d1",
            "execute",
            D1_NAME,
            "--remote",
            "--command",
            "SELECT name FROM sqlite_master WHERE type='table' AND name='items'",
            "--json",
            "--config",
            "wrangler.jsonc",
        ])
        .await;
    assert_success(&migrated);
    assert!(json_contains(
        &json_stdout(&migrated),
        "name",
        &Value::from("items")
    ));
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
    let after_delete = command
        .run(&["d1", "list", "--json", "--config", "wrangler.jsonc"])
        .await;
    assert_success(&after_delete);
    assert!(!json_contains(
        &json_stdout(&after_delete),
        "name",
        &Value::from(D1_NAME)
    ));
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
    fs::write(project.join("r2-input.bin"), b"fixed-wrangler-r2\0payload").unwrap();
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
        fs::read(project.join("r2-output.bin")).unwrap(),
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
    let after_delete = command
        .run(&["queues", "list", "--config", "wrangler.jsonc"])
        .await;
    assert_success(&after_delete);
    assert!(!String::from_utf8_lossy(&after_delete.stdout).contains(QUEUE_NAME));
}

async fn exercise_workflows(command: &WranglerCommand<'_>) {
    let listed = command
        .run(&["workflows", "list", "--config", "wrangler.jsonc"])
        .await;
    assert_success(&listed);
    assert!(String::from_utf8_lossy(&listed.stdout).contains(WORKFLOW_NAME));
    let described = command
        .run(&[
            "workflows",
            "describe",
            WORKFLOW_NAME,
            "--config",
            "wrangler.jsonc",
        ])
        .await;
    assert_success(&described);
    let described = String::from_utf8_lossy(&described.stdout);
    assert!(described.contains(WORKFLOW_NAME));
    assert!(described.contains("resource-gate-worker"));
    assert!(described.contains("ResourceFlow"));
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
    let after_delete = command
        .run(&["workflows", "list", "--config", "wrangler.jsonc"])
        .await;
    assert_success(&after_delete);
    assert!(!String::from_utf8_lossy(&after_delete.stdout).contains(WORKFLOW_NAME));
}

struct Fixture {
    process: platform_process::Process,
    _mock: MockS3,
    _evidence: Evidence,
    _embedding: search::EmbeddingFixture,
    project: PathBuf,
    public_addr: SocketAddr,
    admin_addr: SocketAddr,
    public_account: String,
    log: PathBuf,
}

impl Fixture {
    async fn new() -> Self {
        let repo = repo_root();
        let workerd = PathBuf::from(
            std::env::var_os("OPEN_COMPUTE_TEST_WORKERD")
                .expect("OPEN_COMPUTE_TEST_WORKERD must name the verified stock runtime"),
        );
        assert!(workerd.is_absolute());
        assert!(workerd.is_file());
        verify_runtime_binary(
            &repo.join("packages/runtime/workerd.lock.json"),
            &workerd,
            Duration::from_secs(10),
            &Redactor::new(),
        )
        .await
        .expect("formal pinned stock runtime");

        let runs = repo.join(".temp/p6-wrangler-resource-run");
        fs::create_dir_all(&runs).unwrap();
        let temp = tempfile::Builder::new()
            .prefix("resources-")
            .tempdir_in(runs)
            .unwrap();
        let evidence = Evidence::new(temp);
        let root = evidence.path().to_owned();
        let data = root.join("data");
        let storage = PlatformStorage::bootstrap(&storage_config(&data), &SystemClock).unwrap();
        seed_workflow(&storage);
        drop(storage);

        let mock = MockS3::spawn("open-compute").await;
        let embedding = search::EmbeddingFixture::spawn().await;
        let (public_addr, admin_addr) = platform_process::distinct_addresses();
        let config =
            platform_process::config(&root, &data, &mock.endpoint, public_addr, admin_addr);
        append_resource_config(&config, &root, &embedding.base_url);

        let log = root.join("ocd.stderr.log");
        let mut process = platform_process::spawn(&config, &log);
        let client =
            hyper_util::client::legacy::Client::builder(hyper_util::rt::TokioExecutor::new())
                .build_http();
        wait_ready(&client, admin_addr, &mut process, &log).await;
        let public_account = discover_public_account(&client, admin_addr).await;

        let project = root.join("wrangler-project");
        fs::create_dir(&project).unwrap();
        fs::create_dir(project.join("xdg")).unwrap();
        fs::write(project.join("index.ts"), "export default { fetch() {} };").unwrap();
        write_config(&project, &public_account, None, None);
        Self {
            process,
            _mock: mock,
            _evidence: evidence,
            _embedding: embedding,
            project,
            public_addr,
            admin_addr,
            public_account,
            log,
        }
    }
}

async fn discover_public_account(
    client: &platform_process::Client,
    admin_addr: SocketAddr,
) -> String {
    let request = Request::builder()
        .uri(format!("http://{admin_addr}/client/v4/accounts"))
        .header("authorization", format!("Bearer {READ_ONLY_TOKEN}"))
        .body(Body::empty())
        .unwrap();
    let response = tokio::time::timeout(Duration::from_secs(3), client.request(request))
        .await
        .expect("public account discovery timed out")
        .expect("public account discovery request failed");
    assert_eq!(response.status(), 200);
    let body = to_bytes(Body::new(response.into_body()), 64 * 1024)
        .await
        .unwrap();
    let envelope: Value = serde_json::from_slice(&body).unwrap();
    let accounts = envelope["result"].as_array().unwrap();
    assert_eq!(accounts.len(), 1);
    let public_account = accounts[0]["id"].as_str().unwrap();
    assert!(
        public_account.len() == 32 && public_account.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "v4 account discovery must return one stable 32-hex public ID"
    );
    public_account.to_owned()
}

async fn wait_ready(
    client: &platform_process::Client,
    admin_addr: SocketAddr,
    process: &mut platform_process::Process,
    log: &Path,
) {
    let deadline = Instant::now() + Duration::from_secs(45);
    loop {
        if process.0.try_wait().unwrap().is_some() {
            let log = fs::read(log).unwrap_or_default();
            assert_clean_output(&log);
            panic!(
                "ocd exited before readiness: {}",
                String::from_utf8_lossy(&log)
            );
        }
        if platform_process::response(client, admin_addr, "/health/ready", "GET")
            .await
            .is_ok_and(|response| response.status() == 200)
        {
            return;
        }
        if Instant::now() >= deadline {
            let log = fs::read(log).unwrap_or_default();
            assert_clean_output(&log);
            panic!(
                "ocd readiness timed out; retained sanitized failure evidence; stderr={}",
                String::from_utf8_lossy(&log)
            );
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn append_resource_config(config: &Path, root: &Path, embedding_base_url: &str) {
    let deployer = root.join("deployer.token");
    let read_only = root.join("read-only.token");
    write_token(&deployer, TOKEN);
    write_token(&read_only, READ_ONLY_TOKEN);
    let mut file = fs::OpenOptions::new().append(true).open(config).unwrap();
    writeln!(
        file,
        r#"
[server.deployer_auth]
file = "{}"

[server.read_only_auth]
file = "{}"

{}"#,
        deployer.display(),
        read_only.display(),
        search::ai_config_toml(embedding_base_url),
    )
    .unwrap();
}

fn write_token(path: &Path, value: &str) {
    fs::write(path, format!("{value}\n")).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
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
    fs::write(
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
    let lock = fs::read_to_string(root.join("bun.lock")).unwrap();
    assert!(lock.contains("\"wrangler\": [\"wrangler@4.127.1\""));
    let prefix = format!("wrangler@{WRANGLER_VERSION}+");
    let mut installs = fs::read_dir(root.join("node_modules/.bun"))
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
        serde_json::from_slice(&fs::read(package.join("package.json")).unwrap()).unwrap();
    assert_eq!(metadata["version"], WRANGLER_VERSION);
    package.join("bin/wrangler.js")
}

fn assert_success(output: &Output) {
    assert_clean_output(&output.stdout);
    assert_clean_output(&output.stderr);
    assert!(
        output.status.success(),
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn assert_clean_output(bytes: &[u8]) {
    let text = String::from_utf8_lossy(bytes);
    for secret in evidence::known_secrets() {
        assert!(!text.contains(secret));
    }
    assert!(!text.contains("api.cloudflare.com"));
}

fn json_stdout(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "Wrangler JSON output was invalid: {error}: {}",
            String::from_utf8_lossy(&output.stdout)
        )
    })
}

fn json_contains(value: &Value, key: &str, expected: &Value) -> bool {
    match value {
        Value::Object(object) => {
            object.get(key) == Some(expected)
                || object
                    .values()
                    .any(|value| json_contains(value, key, expected))
        }
        Value::Array(values) => values
            .iter()
            .any(|value| json_contains(value, key, expected)),
        _ => false,
    }
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

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_owned()
}
