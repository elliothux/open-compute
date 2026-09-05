//! Official Cloudflare SDK 7.1.0 against one ready production `ocd` process.

#![cfg(feature = "test-support")]

#[allow(
    dead_code,
    reason = "this Gate reuses only the process half of the shared fixture"
)]
#[path = "workflow_support/platform_process.rs"]
mod platform_process;

use open_compute_artifacts::MockS3;
use open_compute_core::config::DataConfig;
use open_compute_core::{Redactor, RequestId, SecretBytes, SystemClock, VersionId};
use open_compute_runtime::verify_runtime_binary;
use open_compute_storage::{
    NewVersion, NewVersionProducts, PlatformStorage, StoredVersionSecret, VersionContentKind,
    WorkerRepository, WorkflowRepository,
};
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs;
use std::net::SocketAddr;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

const SDK_VERSION: &str = "7.1.0";
const TOKEN: &str = platform_process::ADMIN_TOKEN;
const DEPLOYER_TOKEN: &str = "p6-cloudflare-sdk-deployer-token";
const READ_ONLY_TOKEN: &str = "p6-cloudflare-sdk-read-only-token";
const WORKER_NAME: &str = "sdk-worker";
const WORKFLOW_NAME: &str = "sdk-workflow";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn official_cloudflare_sdk_matches_live_ocd_contract() {
    let mut fixture = Fixture::new().await;
    let sdk = fixed_cloudflare_sdk();
    let output = Command::new("bun")
        .arg("tests/live-router.mjs")
        .current_dir(repo_root().join("packages/cloudflare-extension"))
        .env(
            "OPEN_COMPUTE_V4_BASE_URL",
            format!("http://{}/client/v4", fixture.admin_addr),
        )
        .env("OPEN_COMPUTE_V4_TOKEN", TOKEN)
        .env("OPEN_COMPUTE_CLOUDFLARE_SDK_ENTRY", sdk.join("index.mjs"))
        .env("HTTP_PROXY", "http://127.0.0.1:9")
        .env("HTTPS_PROXY", "http://127.0.0.1:9")
        .env("NO_PROXY", "127.0.0.1,localhost")
        .output()
        .expect("run official Cloudflare SDK contract");
    assert!(
        output.status.success(),
        "stdout={}\nstderr={}\nocd={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
        fs::read_to_string(&fixture.log).unwrap_or_default(),
    );
    assert!(
        fixture.process.0.try_wait().unwrap().is_none(),
        "ocd exited after the SDK contract: {}",
        fs::read_to_string(&fixture.log).unwrap_or_default(),
    );
    fixture.process.stop().await;
    assert!(
        tokio::net::TcpStream::connect(fixture.admin_addr)
            .await
            .is_err()
    );
    assert!(
        tokio::net::TcpStream::connect(fixture.public_addr)
            .await
            .is_err()
    );
    let process_log = fs::read_to_string(&fixture.log).unwrap_or_default();
    for text in [
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
        process_log,
    ] {
        for secret in [TOKEN, DEPLOYER_TOKEN, READ_ONLY_TOKEN] {
            assert!(!text.contains(secret));
        }
        assert!(!text.contains("api.cloudflare.com"));
    }
}

struct Fixture {
    process: platform_process::Process,
    _mock: MockS3,
    _evidence: platform_process::Evidence,
    public_addr: SocketAddr,
    admin_addr: SocketAddr,
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
            Duration::from_secs(20),
            &Redactor::new(),
        )
        .await
        .expect("formal pinned stock runtime");

        let runs = repo.join(".temp/p6-cloudflare-sdk-run");
        fs::create_dir_all(&runs).unwrap();
        let temp = tempfile::Builder::new()
            .prefix("sdk-")
            .tempdir_in(runs)
            .unwrap();
        let root = temp.path().to_owned();
        let data = root.join("data");
        let storage = PlatformStorage::bootstrap(&storage_config(&data), &SystemClock).unwrap();
        seed_worker_and_workflow(&storage);
        drop(storage);

        let mock = MockS3::spawn("open-compute").await;
        let (public_addr, admin_addr) = platform_process::distinct_addresses();
        let config =
            platform_process::config(&root, &data, &mock.endpoint, public_addr, admin_addr);
        append_role_tokens(&root);
        let log = root.join("ocd.stderr.log");
        let mut process = platform_process::spawn(&config, &log);
        let client =
            hyper_util::client::legacy::Client::builder(hyper_util::rt::TokioExecutor::new())
                .build_http();
        platform_process::ready(&client, admin_addr, &mut process).await;
        Self {
            process,
            _mock: mock,
            _evidence: platform_process::Evidence(Some(temp)),
            public_addr,
            admin_addr,
            log,
        }
    }
}

fn append_role_tokens(root: &Path) {
    // platform_process::config already declares the three auth tables; only replace
    // the secret file contents so this Gate can assert its own token values.
    write_token(&root.join("deployer.token"), DEPLOYER_TOKEN);
    write_token(&root.join("read-only.token"), READ_ONLY_TOKEN);
}

fn write_token(path: &Path, value: &str) {
    fs::write(path, format!("{value}\n")).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
}

fn seed_worker_and_workflow(storage: &PlatformStorage) {
    let account = storage.identity().default_account_id;
    let workers = WorkerRepository::new(storage.db());
    let (worker, _) = workers
        .create_worker(account, WORKER_NAME, RequestId::generate(), 1, 1_000_000)
        .unwrap();
    let version = VersionId::generate();
    let revision = uuid::Uuid::now_v7().to_string();
    let envelope = storage
        .crypto()
        .encrypt(
            &SecretBytes::new(b"sdk-secret-value".to_vec()),
            account,
            worker.id,
            version,
            "SDK_SECRET",
            &revision,
        )
        .unwrap();
    let mut secrets = BTreeMap::new();
    secrets.insert(
        "SDK_SECRET".to_owned(),
        StoredVersionSecret {
            name: "SDK_SECRET".to_owned(),
            revision_id: revision,
            envelope,
        },
    );
    workers
        .insert_staging_version(
            &NewVersion {
                id: version,
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
                secrets,
                request_id: RequestId::generate(),
                now_ms: 2,
            },
            &NewVersionProducts::default(),
            1_000_000,
        )
        .unwrap();
    workers.begin_validation(version).unwrap();
    workers.mark_ready(version, 3).unwrap();
    workers
        .promote(account, worker.id, version, None, RequestId::generate(), 4)
        .unwrap();
    let workflows = WorkflowRepository::new(storage.db());
    let definition = workflows
        .create_definition(account, WORKFLOW_NAME, 5)
        .unwrap();
    let workflow_version = workflows
        .stage_version(account, definition.id, version, "SdkWorkflow", 6)
        .unwrap();
    workflows
        .finish_version(
            account,
            workflow_version.target.workflow_version_id,
            true,
            7,
        )
        .unwrap();
}

fn fixed_cloudflare_sdk() -> PathBuf {
    let root = repo_root();
    let lock = fs::read_to_string(root.join("bun.lock")).unwrap();
    assert!(lock.contains("\"cloudflare\": [\"cloudflare@7.1.0\""));
    let prefix = format!("cloudflare@{SDK_VERSION}");
    let mut installs = fs::read_dir(root.join("node_modules/.bun"))
        .expect("locked Bun dependencies must already be installed")
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name().to_string_lossy().starts_with(&prefix))
        .map(|entry| entry.path().join("node_modules/cloudflare"))
        .collect::<Vec<_>>();
    installs.sort();
    installs.dedup();
    assert_eq!(installs.len(), 1, "exactly one fixed Cloudflare SDK");
    let metadata: Value =
        serde_json::from_slice(&fs::read(installs[0].join("package.json")).unwrap()).unwrap();
    assert_eq!(metadata["version"], SDK_VERSION);
    installs.remove(0)
}

fn storage_config(root: &Path) -> DataConfig {
    DataConfig {
        path: root.to_owned(),
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
