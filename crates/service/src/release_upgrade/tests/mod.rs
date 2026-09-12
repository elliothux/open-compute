use super::*;
use crate::install_receipt::{
    InstallReceipt, RECEIPT_SCHEMA_VERSION, path_looks_package_manager_owned, read_receipt,
    write_receipt,
};
use crate::instance_registry::{InstanceRegistry, ServiceScope};
use crate::service_manager::FakeServiceManager;
use open_compute_core::ErrorCode;
use sha2::{Digest, Sha256};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use tempfile::TempDir;

fn host_target() -> &'static str {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => "darwin-arm64",
        ("linux", "x86_64") => "linux-x64",
        ("linux", "aarch64") => "linux-arm64",
        _ => "darwin-arm64",
    }
}

fn fake_binary(version: &str) -> Vec<u8> {
    format!("#!/bin/sh\necho \"ocd {version}\"\n").into_bytes()
}

fn write_loadable_config(dir: &Path) -> PathBuf {
    let dir = dir.canonicalize().unwrap();
    let data = dir.join("data");
    let objects = dir.join("objects");
    let admin = dir.join("admin.token");
    let deployer = dir.join("deployer.token");
    let read_only = dir.join("read-only.token");
    let master = dir.join("master.key");
    fs::create_dir_all(&data).unwrap();
    fs::create_dir_all(&objects).unwrap();
    for (path, value) in [
        (&admin, "admin-secret-value\n"),
        (&deployer, "deployer-secret-value\n"),
        (&read_only, "read-only-secret-value\n"),
    ] {
        fs::write(path, value).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
    }
    let body = format!(
        r#"
[server]
public_bind = "127.0.0.1:0"
admin_auth = {{ file = "{admin}" }}
deployer_auth = {{ file = "{deployer}" }}
read_only_auth = {{ file = "{read_only}" }}

[data]
path = "{data}"
master_key_file = "{master}"

[storage]
backend = "local"
path = "{objects}"
prefix = "system/"

[cache]
max_bytes = 1048576
high_watermark_ratio = 0.9
low_watermark_ratio = 0.8
max_artifact_bytes = 65536
"#,
        admin = admin.display(),
        deployer = deployer.display(),
        read_only = read_only.display(),
        data = data.display(),
        master = master.display(),
        objects = objects.display(),
    );
    let path = dir.join("compute.toml");
    fs::write(&path, body).unwrap();
    path
}

fn fixture_release(
    http: &FixtureReleaseHttp,
    download_base: &str,
    api_base: &str,
    version: &str,
    target: &str,
    binary: &[u8],
) {
    let tag = format!("v{version}");
    let filename = format!("ocd-{tag}-{target}");
    let digest = hex::encode(Sha256::digest(binary));
    let (os, arch) = match target {
        "darwin-arm64" => ("darwin", "arm64"),
        "linux-x64" => ("linux", "x64"),
        "linux-arm64" => ("linux", "arm64"),
        _ => ("darwin", "arm64"),
    };
    let manifest = serde_json::json!({
        "schemaVersion": 1,
        "tag": tag,
        "version": version,
        "gitRevision": "abc123",
        "workerdRelease": "1.0.0",
        "workerdLockSha256": "cd".repeat(32),
        "artifacts": [{
            "target": target,
            "os": os,
            "arch": arch,
            "filename": filename,
            "bytes": binary.len(),
            "sha256": digest,
        }]
    });
    let manifest_bytes = serde_json::to_vec_pretty(&manifest).unwrap();
    let manifest_digest = hex::encode(Sha256::digest(&manifest_bytes));
    let sums = format!("{digest}  {filename}\n{manifest_digest}  release.json\n");
    let base = format!("{download_base}/{tag}");
    http.insert(
        format!("{api_base}/repos/elliothux/open-compute/releases/latest"),
        format!(r#"{{"tag_name":"{tag}","prerelease":false,"draft":false}}"#),
    );
    http.insert(format!("{base}/release.json"), manifest_bytes);
    http.insert(format!("{base}/SHA256SUMS"), sums.into_bytes());
    http.insert(format!("{base}/{filename}"), binary.to_vec());
}

mod dry_run_with_fixture_metadata;

mod uninstall_refuses_registered_instance;

mod package_manager_receipt_blocks_upgrade_options_path;

mod dry_run_rejects_downgrade;

fn base_options(
    _temp: &TempDir,
    binary_path: &Path,
    receipt_path: PathBuf,
    version: Option<&str>,
    dry_run: bool,
    no_restart: bool,
    current: &str,
) -> UpgradeOptions {
    UpgradeOptions {
        version: version.map(str::to_owned),
        dry_run,
        no_restart,
        binary_path: binary_path.to_path_buf(),
        receipt_path,
        staging_dir: binary_path.parent().unwrap().to_path_buf(),
        download_base: "https://fixture.test/download".to_owned(),
        api_base: "https://fixture.test/api".to_owned(),
        target: host_target().to_owned(),
        current_version: current.to_owned(),
    }
}

fn write_upgradeable_pair(temp: &TempDir, version: &str) -> (PathBuf, PathBuf, Vec<u8>) {
    let bin_dir = temp.path().join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let binary_path = bin_dir.join("ocd");
    let current = fake_binary(version);
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o755)
        .open(&binary_path)
        .unwrap();
    file.write_all(&current).unwrap();
    drop(file);
    let receipt_path = temp.path().join("share/open-compute/install-receipt.json");
    write_receipt(
        &receipt_path,
        &InstallReceipt {
            schema_version: RECEIPT_SCHEMA_VERSION,
            version: version.to_owned(),
            sha256: hex::encode(Sha256::digest(&current)),
            target: host_target().to_owned(),
            binary_path: binary_path.to_string_lossy().into_owned(),
            method: "install.sh".to_owned(),
            source: "test://current".to_owned(),
            installed_at_ms: 1,
        },
    )
    .unwrap();
    (binary_path, receipt_path, current)
}

mod upgrade_replaces_binary_and_restarts_instances;

mod upgrade_preserves_stopped_instance_state;

mod upgrade_validates_registered_configs_before_binary_replace;

mod upgrade_no_restart_skips_manager;

mod upgrade_restart_failure_stops_remaining;

mod resolve_release_rejects_bad_inputs;

mod resolve_release_rejects_checksum_mismatch;

mod check_upgrade_available_reports_newer;

mod uninstall_removes_binary_and_receipt;

mod dry_run_rejects_already_installed;

mod fixture_http_enforces_size_bound_and_missing_url;

mod resolve_release_rejects_prerelease_and_bad_manifest;

mod upgrade_rejects_size_and_checksum_mismatch;

mod uninstall_rejects_receipt_binary_mismatch;

mod upgrade_options_production_and_load_receipt;

mod check_upgrade_available_when_blocked_and_current;

mod check_upgrade_available_propagates_resolve_failures;

mod resolve_latest_stable_tag_fail_closed_branches;

mod resolve_release_rejects_manifest_and_sums_corruption;

mod upgrade_rejects_equal_version_and_staged_version_mismatch;

mod write_staged_binary_and_atomic_replace_fail_closed;

mod verify_staged_version_rejects_bad_executables;

mod dry_run_rejects_unstable_current_version;

mod upgrade_options_production_resolves_on_host;

mod uninstall_rejects_package_manager_owned_path;

mod upgrade_rejects_artifact_checksum_mismatch;

mod parse_sha256sums_skips_blank_lines_and_rejects_empty_fields;

mod atomic_replace_rename_failure_and_failing_writer;
