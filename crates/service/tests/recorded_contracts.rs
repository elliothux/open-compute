//! Portable Cloudflare recordings against an isolated, offline local daemon.
#![cfg(feature = "test-support")]

#[allow(dead_code, reason = "this Gate uses only process startup and shutdown")]
#[path = "workflow_support/platform_process.rs"]
mod platform_process;

use open_compute_artifacts::MockS3;
use open_compute_core::{SystemClock, config::DataConfig};
use open_compute_storage::PlatformStorage;
use std::path::Path;
use std::time::Duration;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn portable_recordings_match_local_ocd() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let temp = tempfile::tempdir().unwrap();
    let data = temp.path().join("data");
    let storage = PlatformStorage::bootstrap(
        &DataConfig {
            path: data.clone(),
            master_key_file: data.join("keys/master.key"),
            master_key_env: None,
            sqlite_busy_timeout_ms: 5_000,
            free_space_soft_bytes: 1_073_741_824,
            free_space_hard_bytes: 1,
        },
        &SystemClock,
    )
    .unwrap();
    let account = storage.identity().instance_id.to_string();
    drop(storage);
    let mock = MockS3::spawn("open-compute").await;
    let (public, admin) = platform_process::distinct_addresses();
    let config = platform_process::config(temp.path(), &data, &mock.endpoint, public, admin);
    let log = temp.path().join("ocd.log");
    let mut process = platform_process::spawn(&config, &log);
    let client = hyper_util::client::legacy::Client::builder(hyper_util::rt::TokioExecutor::new())
        .build_http();
    platform_process::ready(&client, admin, &mut process).await;
    let mut command = tokio::process::Command::new("bun");
    command.env_clear();
    for key in ["PATH", "TMPDIR", "TMP", "TEMP"] {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
    let output = tokio::time::timeout(
        Duration::from_secs(1200),
        command
            .current_dir(&root)
            .args(["test/conformance/differential.ts", "--local"])
            .env("HOME", temp.path().join("home"))
            .env("OPEN_COMPUTE_ENDPOINT", format!("http://{admin}"))
            .env("OPEN_COMPUTE_PUBLIC_ENDPOINT", format!("http://{public}"))
            .env("OPEN_COMPUTE_ACCOUNT_ID", account)
            .env("OPEN_COMPUTE_ADMIN_TOKEN", "workflow-deployer")
            .kill_on_drop(true)
            .output(),
    )
    .await
    .unwrap()
    .unwrap();
    process.stop().await;
    assert!(
        !output
            .stdout
            .windows(b"workflow-deployer".len())
            .any(|value| value == b"workflow-deployer")
    );
    assert!(
        output.status.success(),
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
