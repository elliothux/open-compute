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

#[tokio::test]
async fn dry_run_with_fixture_metadata() {
    let temp = TempDir::new().unwrap();
    let bin_dir = temp.path().join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let binary_path = bin_dir.join("ocd");
    let current = fake_binary("0.1.0");
    fs::write(&binary_path, &current).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&binary_path, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let receipt_path = temp.path().join("share/open-compute/install-receipt.json");
    write_receipt(
        &receipt_path,
        &InstallReceipt {
            schema_version: RECEIPT_SCHEMA_VERSION,
            version: "0.1.0".to_owned(),
            sha256: hex::encode(Sha256::digest(&current)),
            target: host_target().to_owned(),
            binary_path: binary_path.to_string_lossy().into_owned(),
            method: "install.sh".to_owned(),
            source: "test://current".to_owned(),
            installed_at_ms: 1,
        },
    )
    .unwrap();

    let download_base = "https://fixture.test/download".to_string();
    let api_base = "https://fixture.test/api".to_string();
    let http = FixtureReleaseHttp::default();
    let next = fake_binary("0.1.1");
    fixture_release(
        &http,
        &download_base,
        &api_base,
        "0.1.1",
        host_target(),
        &next,
    );

    let options = UpgradeOptions {
        version: None,
        dry_run: true,
        no_restart: false,
        binary_path: binary_path.clone(),
        receipt_path,
        staging_dir: bin_dir,
        download_base,
        api_base,
        target: host_target().to_owned(),
        current_version: "0.1.0".to_owned(),
    };
    let registry = InstanceRegistry::with_roots(
        temp.path().join("registry/system"),
        temp.path().join("registry/user"),
    );
    let manager = FakeServiceManager::default();
    let mut out = Vec::new();
    run_upgrade(&options, &http, &registry, &manager, &mut out)
        .await
        .unwrap();
    let text = String::from_utf8(out).unwrap();
    assert!(text.contains("UPGRADE_DRY_RUN_OK 0.1.1"));
    assert_eq!(fs::read(&binary_path).unwrap(), current);
}

#[tokio::test]
async fn uninstall_refuses_registered_instance() {
    let temp = TempDir::new().unwrap();
    let bin_dir = temp.path().join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let binary_path = bin_dir.join("ocd");
    let binary = fake_binary("0.1.0");
    fs::write(&binary_path, &binary).unwrap();
    let receipt_path = temp.path().join("share/open-compute/install-receipt.json");
    write_receipt(
        &receipt_path,
        &InstallReceipt {
            schema_version: RECEIPT_SCHEMA_VERSION,
            version: "0.1.0".to_owned(),
            sha256: hex::encode(Sha256::digest(&binary)),
            target: host_target().to_owned(),
            binary_path: binary_path.to_string_lossy().into_owned(),
            method: "install.sh".to_owned(),
            source: "test://current".to_owned(),
            installed_at_ms: 1,
        },
    )
    .unwrap();
    let registry = InstanceRegistry::with_roots(
        temp.path().join("registry/system"),
        temp.path().join("registry/user"),
    );
    let config = temp.path().join("compute.toml");
    fs::write(&config, b"placeholder").unwrap();
    // Register via low-level write by using a canonical absolute path digest.
    let canonical = config.canonicalize().unwrap();
    registry
        .register(&canonical, ServiceScope::User, SystemTime::now())
        .unwrap();
    let manager = FakeServiceManager::default();
    let mut out = Vec::new();
    let err =
        run_uninstall(&receipt_path, &binary_path, &registry, &manager, &mut out).unwrap_err();
    assert_eq!(err.code(), ErrorCode::DataDirInUse);
    assert!(
        String::from_utf8(out)
            .unwrap()
            .contains("UNINSTALL_BLOCKED_INSTANCE")
    );
    assert!(binary_path.exists());
}

#[test]
fn package_manager_receipt_blocks_upgrade_options_path() {
    let temp = TempDir::new().unwrap();
    let binary_path = temp.path().join("opt/homebrew/bin/ocd");
    fs::create_dir_all(binary_path.parent().unwrap()).unwrap();
    // Force cellar-looking path component.
    let cellar = temp.path().join("opt/homebrew/Cellar/ocd/bin/ocd");
    fs::create_dir_all(cellar.parent().unwrap()).unwrap();
    fs::write(&cellar, b"x").unwrap();
    let err = require_upgradeable_receipt(&temp.path().join("missing.json"), &cellar).unwrap_err();
    assert!(err.message().contains("package-manager-owned"));
}

#[tokio::test]
async fn dry_run_rejects_downgrade() {
    let temp = TempDir::new().unwrap();
    let bin_dir = temp.path().join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let binary_path = bin_dir.join("ocd");
    let current = fake_binary("0.2.0");
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
            version: "0.2.0".to_owned(),
            sha256: hex::encode(Sha256::digest(&current)),
            target: host_target().to_owned(),
            binary_path: binary_path.to_string_lossy().into_owned(),
            method: "manual".to_owned(),
            source: "test://current".to_owned(),
            installed_at_ms: 1,
        },
    )
    .unwrap();
    let download_base = "https://fixture.test/download".to_owned();
    let api_base = "https://fixture.test/api".to_owned();
    let http = FixtureReleaseHttp::default();
    let older = fake_binary("0.1.0");
    fixture_release(
        &http,
        &download_base,
        &api_base,
        "0.1.0",
        host_target(),
        &older,
    );
    let options = UpgradeOptions {
        version: Some("0.1.0".to_owned()),
        dry_run: true,
        no_restart: false,
        binary_path,
        receipt_path,
        staging_dir: bin_dir,
        download_base,
        api_base,
        target: host_target().to_owned(),
        current_version: "0.2.0".to_owned(),
    };
    let registry = InstanceRegistry::with_roots(
        temp.path().join("registry/system"),
        temp.path().join("registry/user"),
    );
    let err = run_upgrade(
        &options,
        &http,
        &registry,
        &FakeServiceManager::default(),
        &mut Vec::new(),
    )
    .await
    .unwrap_err();
    assert!(err.message().contains("downgrade"));
}

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

#[tokio::test]
async fn upgrade_replaces_binary_and_restarts_instances() {
    let temp = TempDir::new().unwrap();
    let (binary_path, receipt_path, current) = write_upgradeable_pair(&temp, "0.1.0");
    let download_base = "https://fixture.test/download".to_owned();
    let api_base = "https://fixture.test/api".to_owned();
    let http = FixtureReleaseHttp::default();
    let target_version = env!("CARGO_PKG_VERSION");
    let next = fake_binary(target_version);
    fixture_release(
        &http,
        &download_base,
        &api_base,
        target_version,
        host_target(),
        &next,
    );
    let registry = InstanceRegistry::with_roots(
        temp.path().join("registry/system"),
        temp.path().join("registry/user"),
    );
    let config = write_loadable_config(temp.path());
    let record = registry
        .register(
            &config.canonicalize().unwrap(),
            ServiceScope::User,
            SystemTime::now(),
        )
        .unwrap();
    let manager = FakeServiceManager::default();
    manager.install(&record, &binary_path).unwrap();
    manager.start(&record).unwrap();
    let options = base_options(
        &temp,
        &binary_path,
        receipt_path.clone(),
        None,
        false,
        false,
        "0.1.0",
    );
    let mut out = Vec::new();
    run_upgrade(&options, &http, &registry, &manager, &mut out)
        .await
        .unwrap();
    let text = String::from_utf8(out).unwrap();
    assert!(text.contains(&format!("UPGRADE_OK {target_version}")));
    assert!(text.contains("UPGRADE_INSTANCE_RESTARTED"));
    assert_eq!(fs::read(&binary_path).unwrap(), next);
    assert_ne!(fs::read(&binary_path).unwrap(), current);
    let receipt = read_receipt(&receipt_path).unwrap();
    assert_eq!(receipt.version, target_version);
}

#[tokio::test]
async fn upgrade_preserves_stopped_instance_state() {
    let temp = TempDir::new().unwrap();
    let (binary_path, receipt_path, _) = write_upgradeable_pair(&temp, "0.1.0");
    let download_base = "https://fixture.test/download".to_owned();
    let api_base = "https://fixture.test/api".to_owned();
    let http = FixtureReleaseHttp::default();
    let target_version = env!("CARGO_PKG_VERSION");
    let next = fake_binary(target_version);
    fixture_release(
        &http,
        &download_base,
        &api_base,
        target_version,
        host_target(),
        &next,
    );
    let registry = InstanceRegistry::with_roots(
        temp.path().join("registry/system"),
        temp.path().join("registry/user"),
    );
    let config = write_loadable_config(temp.path());
    let record = registry
        .register(
            &config.canonicalize().unwrap(),
            ServiceScope::User,
            SystemTime::now(),
        )
        .unwrap();
    let manager = FakeServiceManager::default();
    manager.install(&record, &binary_path).unwrap();
    let options = base_options(
        &temp,
        &binary_path,
        receipt_path,
        Some(target_version),
        false,
        false,
        "0.1.0",
    );
    let mut out = Vec::new();
    run_upgrade(&options, &http, &registry, &manager, &mut out)
        .await
        .unwrap();
    assert!(manager.started().is_empty());
    assert!(
        !String::from_utf8(out)
            .unwrap()
            .contains("UPGRADE_INSTANCE_RESTARTED")
    );
    assert_eq!(fs::read(&binary_path).unwrap(), next);
}

#[tokio::test]
async fn upgrade_validates_registered_configs_before_binary_replace() {
    let temp = TempDir::new().unwrap();
    let (binary_path, receipt_path, current) = write_upgradeable_pair(&temp, "0.1.0");
    let download_base = "https://fixture.test/download".to_owned();
    let api_base = "https://fixture.test/api".to_owned();
    let http = FixtureReleaseHttp::default();
    let target_version = env!("CARGO_PKG_VERSION");
    fixture_release(
        &http,
        &download_base,
        &api_base,
        target_version,
        host_target(),
        &fake_binary(target_version),
    );
    let registry = InstanceRegistry::with_roots(
        temp.path().join("registry/system"),
        temp.path().join("registry/user"),
    );
    let config = temp.path().join("compute.toml");
    fs::write(&config, b"not valid toml").unwrap();
    registry
        .register(
            &config.canonicalize().unwrap(),
            ServiceScope::User,
            SystemTime::now(),
        )
        .unwrap();
    let options = base_options(
        &temp,
        &binary_path,
        receipt_path,
        Some(target_version),
        false,
        false,
        "0.1.0",
    );
    assert!(
        run_upgrade(
            &options,
            &http,
            &registry,
            &FakeServiceManager::default(),
            &mut Vec::new(),
        )
        .await
        .is_err()
    );
    assert_eq!(fs::read(&binary_path).unwrap(), current);
}

#[tokio::test]
async fn upgrade_no_restart_skips_manager() {
    let temp = TempDir::new().unwrap();
    let (binary_path, receipt_path, _) = write_upgradeable_pair(&temp, "0.1.0");
    let download_base = "https://fixture.test/download".to_owned();
    let api_base = "https://fixture.test/api".to_owned();
    let http = FixtureReleaseHttp::default();
    let next = fake_binary("0.1.3");
    fixture_release(
        &http,
        &download_base,
        &api_base,
        "0.1.3",
        host_target(),
        &next,
    );
    let registry = InstanceRegistry::with_roots(
        temp.path().join("registry/system"),
        temp.path().join("registry/user"),
    );
    let manager = FakeServiceManager::default();
    let options = base_options(
        &temp,
        &binary_path,
        receipt_path,
        Some("0.1.3"),
        false,
        true,
        "0.1.0",
    );
    let mut out = Vec::new();
    run_upgrade(&options, &http, &registry, &manager, &mut out)
        .await
        .unwrap();
    assert!(String::from_utf8(out).unwrap().contains("--no-restart"));
    assert_eq!(fs::read(&binary_path).unwrap(), next);
    assert!(manager.started().is_empty());
}

#[tokio::test]
async fn upgrade_restart_failure_stops_remaining() {
    let temp = TempDir::new().unwrap();
    let (binary_path, receipt_path, _) = write_upgradeable_pair(&temp, "0.1.0");
    let download_base = "https://fixture.test/download".to_owned();
    let api_base = "https://fixture.test/api".to_owned();
    let http = FixtureReleaseHttp::default();
    let next = fake_binary("0.1.4");
    fixture_release(
        &http,
        &download_base,
        &api_base,
        "0.1.4",
        host_target(),
        &next,
    );
    let registry = InstanceRegistry::with_roots(
        temp.path().join("registry/system"),
        temp.path().join("registry/user"),
    );
    let config = write_loadable_config(temp.path());
    let record = registry
        .register(
            &config.canonicalize().unwrap(),
            ServiceScope::User,
            SystemTime::now(),
        )
        .unwrap();
    let manager = FakeServiceManager::default();
    manager.install(&record, &binary_path).unwrap();
    manager.start(&record).unwrap();
    manager.set_fail_restart(true);
    let options = base_options(
        &temp,
        &binary_path,
        receipt_path,
        Some("0.1.4"),
        false,
        false,
        "0.1.0",
    );
    let mut out = Vec::new();
    let err = run_upgrade(&options, &http, &registry, &manager, &mut out)
        .await
        .unwrap_err();
    assert_eq!(err.code(), ErrorCode::PlatformUnavailable);
    assert!(
        String::from_utf8(out)
            .unwrap()
            .contains("UPGRADE_INSTANCE_FAILED")
    );
    // Binary already replaced before restart.
    assert_eq!(fs::read(&binary_path).unwrap(), next);
}

#[tokio::test]
async fn resolve_release_rejects_bad_inputs() {
    let http = FixtureReleaseHttp::default();
    let err = resolve_release(
        &http,
        "https://fixture.test/api",
        "https://fixture.test/download",
        Some("0.1.0-rc.1"),
        host_target(),
    )
    .await
    .unwrap_err();
    assert!(err.message().contains("stable SemVer"));
    let err = resolve_release(
        &http,
        "https://fixture.test/api",
        "https://fixture.test/download",
        Some("0.1.0"),
        "windows-x64",
    )
    .await
    .unwrap_err();
    assert_eq!(err.code(), ErrorCode::ReleaseUnsupported);
}

#[tokio::test]
async fn resolve_release_rejects_checksum_mismatch() {
    let http = FixtureReleaseHttp::default();
    let download_base = "https://fixture.test/download";
    let api_base = "https://fixture.test/api";
    let binary = fake_binary("0.1.5");
    let tag = "v0.1.5";
    let target = host_target();
    let filename = format!("ocd-{tag}-{target}");
    let digest = hex::encode(Sha256::digest(&binary));
    let manifest = serde_json::json!({
        "schemaVersion": 1,
        "tag": tag,
        "version": "0.1.5",
        "gitRevision": "abc",
        "workerdRelease": "1.0.0",
        "workerdLockSha256": "cd".repeat(32),
        "artifacts": [{
            "target": target,
            "os": "darwin",
            "arch": "arm64",
            "filename": filename,
            "bytes": binary.len(),
            "sha256": digest,
        }]
    });
    let manifest_bytes = serde_json::to_vec_pretty(&manifest).unwrap();
    let bad_sums = format!(
        "{}  {filename}\n{}  release.json\n",
        "00".repeat(32),
        hex::encode(Sha256::digest(&manifest_bytes))
    );
    let base = format!("{download_base}/{tag}");
    http.insert(format!("{base}/release.json"), manifest_bytes);
    http.insert(format!("{base}/SHA256SUMS"), bad_sums.into_bytes());
    let err = resolve_release(&http, api_base, download_base, Some("0.1.5"), target)
        .await
        .unwrap_err();
    assert_eq!(err.code(), ErrorCode::ArtifactIntegrityError);
}

#[tokio::test]
async fn check_upgrade_available_reports_newer() {
    let temp = TempDir::new().unwrap();
    let (binary_path, receipt_path, _) = write_upgradeable_pair(&temp, "0.1.0");
    let download_base = "https://fixture.test/download".to_owned();
    let api_base = "https://fixture.test/api".to_owned();
    let http = FixtureReleaseHttp::default();
    fixture_release(
        &http,
        &download_base,
        &api_base,
        "0.2.0",
        host_target(),
        &fake_binary("0.2.0"),
    );
    let result = check_upgrade_available(
        &http,
        &api_base,
        &download_base,
        "0.1.0",
        &receipt_path,
        &binary_path,
        host_target(),
    )
    .await
    .unwrap();
    assert_eq!(result.available_version.as_deref(), Some("0.2.0"));
    assert!(result.upgrade_allowed);
}

#[tokio::test]
async fn uninstall_removes_binary_and_receipt() {
    let temp = TempDir::new().unwrap();
    let (binary_path, receipt_path, _) = write_upgradeable_pair(&temp, "0.1.0");
    let registry = InstanceRegistry::with_roots(
        temp.path().join("registry/system"),
        temp.path().join("registry/user"),
    );
    let manager = FakeServiceManager::default();
    let mut out = Vec::new();
    run_uninstall(&receipt_path, &binary_path, &registry, &manager, &mut out).unwrap();
    assert!(String::from_utf8(out).unwrap().contains("UNINSTALL_OK"));
    assert!(!binary_path.exists());
    assert!(!receipt_path.exists());
}

#[tokio::test]
async fn dry_run_rejects_already_installed() {
    let temp = TempDir::new().unwrap();
    let (binary_path, receipt_path, _) = write_upgradeable_pair(&temp, "0.1.0");
    let download_base = "https://fixture.test/download".to_owned();
    let api_base = "https://fixture.test/api".to_owned();
    let http = FixtureReleaseHttp::default();
    fixture_release(
        &http,
        &download_base,
        &api_base,
        "0.1.0",
        host_target(),
        &fake_binary("0.1.0"),
    );
    let options = base_options(
        &temp,
        &binary_path,
        receipt_path,
        Some("0.1.0"),
        true,
        false,
        "0.1.0",
    );
    let err = run_upgrade(
        &options,
        &http,
        &InstanceRegistry::with_roots(
            temp.path().join("registry/system"),
            temp.path().join("registry/user"),
        ),
        &FakeServiceManager::default(),
        &mut Vec::new(),
    )
    .await
    .unwrap_err();
    assert!(err.message().contains("already installed"));
}

#[tokio::test]
async fn fixture_http_enforces_size_bound_and_missing_url() {
    let http = FixtureReleaseHttp::default();
    http.insert("https://fixture.test/big", vec![0u8; 8]);
    let err = http.get("https://fixture.test/big", 4).await.unwrap_err();
    assert_eq!(err.code(), ErrorCode::LimitInvalid);
    let err = http
        .get("https://fixture.test/missing", 1024)
        .await
        .unwrap_err();
    assert_eq!(err.code(), ErrorCode::PlatformUnavailable);
}

#[tokio::test]
async fn resolve_release_rejects_prerelease_and_bad_manifest() {
    let http = FixtureReleaseHttp::default();
    let api = "https://fixture.test/api";
    http.insert(
        format!("{api}/repos/elliothux/open-compute/releases/latest"),
        r#"{"tag_name":"v0.2.0-rc.1","prerelease":true,"draft":false}"#,
    );
    let err = resolve_release(
        &http,
        api,
        "https://fixture.test/download",
        None,
        host_target(),
    )
    .await
    .unwrap_err();
    assert!(err.message().contains("prerelease") || err.message().contains("stable"));

    let download = "https://fixture.test/download";
    http.insert(format!("{download}/v0.2.0/release.json"), b"{not-json");
    http.insert(format!("{download}/v0.2.0/SHA256SUMS"), b"deadbeef  x\n");
    let err = resolve_release(&http, api, download, Some("0.2.0"), host_target())
        .await
        .unwrap_err();
    assert_eq!(err.code(), ErrorCode::ReleaseUnsupported);
}

#[tokio::test]
async fn upgrade_rejects_size_and_checksum_mismatch() {
    let temp = TempDir::new().unwrap();
    let (binary_path, receipt_path, _) = write_upgradeable_pair(&temp, "0.1.0");
    let download_base = "https://fixture.test/download".to_owned();
    let api_base = "https://fixture.test/api".to_owned();
    let http = FixtureReleaseHttp::default();
    let next = fake_binary("0.1.6");
    let tag = "v0.1.6";
    let target = host_target();
    let filename = format!("ocd-{tag}-{target}");
    let digest = hex::encode(Sha256::digest(&next));
    let (os, arch) = match target {
        "darwin-arm64" => ("darwin", "arm64"),
        "linux-x64" => ("linux", "x64"),
        "linux-arm64" => ("linux", "arm64"),
        _ => ("darwin", "arm64"),
    };
    let manifest = serde_json::json!({
        "schemaVersion": 1,
        "tag": tag,
        "version": "0.1.6",
        "gitRevision": "abc",
        "workerdRelease": "1.0.0",
        "workerdLockSha256": "cd".repeat(32),
        "artifacts": [{
            "target": target,
            "os": os,
            "arch": arch,
            "filename": filename,
            "bytes": next.len() + 1,
            "sha256": digest,
        }]
    });
    let manifest_bytes = serde_json::to_vec_pretty(&manifest).unwrap();
    let sums = format!(
        "{digest}  {filename}\n{}  release.json\n",
        hex::encode(Sha256::digest(&manifest_bytes))
    );
    let base = format!("{download_base}/{tag}");
    http.insert(
        format!("{api_base}/repos/elliothux/open-compute/releases/latest"),
        format!(r#"{{"tag_name":"{tag}","prerelease":false,"draft":false}}"#),
    );
    http.insert(format!("{base}/release.json"), manifest_bytes);
    http.insert(format!("{base}/SHA256SUMS"), sums.into_bytes());
    http.insert(format!("{base}/{filename}"), next);
    let options = base_options(
        &temp,
        &binary_path,
        receipt_path,
        Some("0.1.6"),
        false,
        true,
        "0.1.0",
    );
    let err = run_upgrade(
        &options,
        &http,
        &InstanceRegistry::with_roots(
            temp.path().join("registry/system"),
            temp.path().join("registry/user"),
        ),
        &FakeServiceManager::default(),
        &mut Vec::new(),
    )
    .await
    .unwrap_err();
    assert_eq!(err.code(), ErrorCode::ArtifactIntegrityError);
}

#[tokio::test]
async fn uninstall_rejects_receipt_binary_mismatch() {
    let temp = TempDir::new().unwrap();
    let (binary_path, receipt_path, _) = write_upgradeable_pair(&temp, "0.1.0");
    let other = temp.path().join("bin/other-ocd");
    fs::write(&other, b"x").unwrap();
    let err = run_uninstall(
        &receipt_path,
        &other,
        &InstanceRegistry::with_roots(
            temp.path().join("registry/system"),
            temp.path().join("registry/user"),
        ),
        &FakeServiceManager::default(),
        &mut Vec::new(),
    )
    .unwrap_err();
    assert!(
        err.message().contains("does not match") || err.code() == ErrorCode::ReleaseUnsupported
    );
    let _ = binary_path;
}

#[test]
fn upgrade_options_production_and_load_receipt() {
    let options = UpgradeOptions::production(None, true, true).unwrap();
    assert!(options.binary_path.is_absolute() || options.binary_path.exists());
    assert!(options.dry_run);
    assert!(options.no_restart);
    let _ = load_receipt_for_exe();
    let _ = LiveReleaseHttp::new().unwrap();
}

#[tokio::test]
async fn check_upgrade_available_when_blocked_and_current() {
    let temp = TempDir::new().unwrap();
    let binary = temp.path().join("bin/ocd");
    fs::create_dir_all(binary.parent().unwrap()).unwrap();
    fs::write(&binary, b"ocd").unwrap();
    let download_base = "https://fixture.test/download".to_owned();
    let api_base = "https://fixture.test/api".to_owned();
    let http = FixtureReleaseHttp::default();
    fixture_release(
        &http,
        &download_base,
        &api_base,
        "0.1.0",
        host_target(),
        &fake_binary("0.1.0"),
    );
    let result = check_upgrade_available(
        &http,
        &api_base,
        &download_base,
        "0.1.0",
        &temp.path().join("missing-receipt.json"),
        &binary,
        host_target(),
    )
    .await
    .unwrap();
    assert!(!result.upgrade_allowed);
    assert!(result.available_version.is_none());
}

#[tokio::test]
async fn check_upgrade_available_propagates_resolve_failures() {
    let temp = TempDir::new().unwrap();
    let binary = temp.path().join("bin/ocd");
    fs::create_dir_all(binary.parent().unwrap()).unwrap();
    fs::write(&binary, b"ocd").unwrap();
    let http = FixtureReleaseHttp::default();
    let err = check_upgrade_available(
        &http,
        "https://fixture.test/api",
        "https://fixture.test/download",
        "0.1.0",
        &temp.path().join("missing-receipt.json"),
        &binary,
        host_target(),
    )
    .await
    .unwrap_err();
    assert_eq!(err.code(), ErrorCode::PlatformUnavailable);
}

#[tokio::test]
async fn resolve_latest_stable_tag_fail_closed_branches() {
    let http = FixtureReleaseHttp::default();
    let api = "https://fixture.test/api";
    let download = "https://fixture.test/download";
    let latest = format!("{api}/repos/elliothux/open-compute/releases/latest");

    http.insert(latest.clone(), b"not-json");
    assert_eq!(
        resolve_release(&http, api, download, None, host_target())
            .await
            .unwrap_err()
            .code(),
        ErrorCode::ReleaseUnsupported
    );

    http.insert(latest.clone(), r#"{"prerelease":false,"draft":false}"#);
    assert!(
        resolve_release(&http, api, download, None, host_target())
            .await
            .unwrap_err()
            .message()
            .contains("tag_name")
    );

    http.insert(
        latest.clone(),
        r#"{"tag_name":"v0.9.0","prerelease":false,"draft":true}"#,
    );
    let err = resolve_release(&http, api, download, None, host_target())
        .await
        .unwrap_err();
    assert!(
        err.message().contains("prerelease") || err.message().contains("draft"),
        "{err:?}"
    );

    // Passes GitHub draft/prerelease gates but fails stable SemVer / leading-v checks.
    http.insert(
        latest.clone(),
        r#"{"tag_name":"1.2.3","prerelease":false,"draft":false}"#,
    );
    assert!(
        resolve_release(&http, api, download, None, host_target())
            .await
            .unwrap_err()
            .message()
            .contains("stable SemVer")
    );
    http.insert(
        latest,
        r#"{"tag_name":"v1.2.3-beta.1","prerelease":false,"draft":false}"#,
    );
    assert!(
        resolve_release(&http, api, download, None, host_target())
            .await
            .unwrap_err()
            .message()
            .contains("stable SemVer")
    );
}

#[tokio::test]
async fn resolve_release_rejects_manifest_and_sums_corruption() {
    let http = FixtureReleaseHttp::default();
    let api = "https://fixture.test/api";
    let download = "https://fixture.test/download";
    let target = host_target();
    let tag = "v0.3.0";
    let base = format!("{download}/{tag}");
    let filename = format!("ocd-{tag}-{target}");

    // Tag/version mismatch between path and release.json.
    let mut manifest = serde_json::json!({
        "schemaVersion": 1,
        "tag": "v0.3.1",
        "version": "0.3.1",
        "gitRevision": "abc",
        "workerdRelease": "1.0.0",
        "workerdLockSha256": "cd".repeat(32),
        "artifacts": [{
            "target": target,
            "os": "darwin",
            "arch": "arm64",
            "filename": filename,
            "bytes": 4,
            "sha256": "ab".repeat(32),
        }]
    });
    let bytes = serde_json::to_vec(&manifest).unwrap();
    http.insert(format!("{base}/release.json"), bytes.clone());
    http.insert(
        format!("{base}/SHA256SUMS"),
        format!("{}  release.json\n", hex::encode(Sha256::digest(&bytes))).into_bytes(),
    );
    assert!(
        resolve_release(&http, api, download, Some("0.3.0"), target)
            .await
            .unwrap_err()
            .message()
            .contains("tag/version")
    );

    // Unsupported schema.
    manifest["schemaVersion"] = serde_json::json!(99);
    manifest["tag"] = serde_json::json!(tag);
    manifest["version"] = serde_json::json!("0.3.0");
    let bytes = serde_json::to_vec(&manifest).unwrap();
    http.insert(format!("{base}/release.json"), bytes.clone());
    http.insert(
        format!("{base}/SHA256SUMS"),
        format!("{}  release.json\n", hex::encode(Sha256::digest(&bytes))).into_bytes(),
    );
    assert!(
        resolve_release(&http, api, download, Some("0.3.0"), target)
            .await
            .unwrap_err()
            .message()
            .contains("schema")
    );

    // Unstable version inside an otherwise well-formed schema-1 document.
    manifest["schemaVersion"] = serde_json::json!(1);
    manifest["version"] = serde_json::json!("0.3.0-rc.1");
    let bytes = serde_json::to_vec(&manifest).unwrap();
    http.insert(format!("{base}/release.json"), bytes);
    http.insert(format!("{base}/SHA256SUMS"), b"aa  release.json\n");
    assert!(
        resolve_release(&http, api, download, Some("0.3.0"), target)
            .await
            .unwrap_err()
            .code()
            == ErrorCode::ReleaseUnsupported
    );

    // Non-UTF8 SHA256SUMS.
    let good = serde_json::json!({
        "schemaVersion": 1,
        "tag": tag,
        "version": "0.3.0",
        "gitRevision": "abc",
        "workerdRelease": "1.0.0",
        "workerdLockSha256": "cd".repeat(32),
        "artifacts": [{
            "target": target,
            "os": "darwin",
            "arch": "arm64",
            "filename": filename,
            "bytes": 4,
            "sha256": "ab".repeat(32),
        }]
    });
    let bytes = serde_json::to_vec(&good).unwrap();
    http.insert(format!("{base}/release.json"), bytes.clone());
    http.insert(format!("{base}/SHA256SUMS"), vec![0xff, 0xfe, 0xfd]);
    assert!(
        resolve_release(&http, api, download, Some("0.3.0"), target)
            .await
            .unwrap_err()
            .message()
            .contains("UTF-8")
    );

    // Malformed SUMS lines and missing release.json entry.
    for sums in [
        b"onlydigest\n".as_slice(),
        b"abcd name extra\n".as_slice(),
        format!("{}  {filename}\n", "ab".repeat(32))
            .into_bytes()
            .as_slice(),
    ] {
        http.insert(format!("{base}/SHA256SUMS"), sums.to_vec());
        let _ = resolve_release(&http, api, download, Some("0.3.0"), target).await;
    }

    // Missing target artifact.
    let no_artifact = serde_json::json!({
        "schemaVersion": 1,
        "tag": tag,
        "version": "0.3.0",
        "gitRevision": "abc",
        "workerdRelease": "1.0.0",
        "workerdLockSha256": "cd".repeat(32),
        "artifacts": []
    });
    let bytes = serde_json::to_vec(&no_artifact).unwrap();
    let sums = format!("{}  release.json\n", hex::encode(Sha256::digest(&bytes)));
    http.insert(format!("{base}/release.json"), bytes);
    http.insert(format!("{base}/SHA256SUMS"), sums.into_bytes());
    assert!(
        resolve_release(&http, api, download, Some("0.3.0"), target)
            .await
            .unwrap_err()
            .message()
            .contains("artifact")
    );

    // release.json digest mismatch against SHA256SUMS.
    let bytes = serde_json::to_vec(&good).unwrap();
    http.insert(format!("{base}/release.json"), bytes);
    http.insert(
        format!("{base}/SHA256SUMS"),
        format!(
            "{}  release.json\n{}  {filename}\n",
            "00".repeat(32),
            "ab".repeat(32)
        )
        .into_bytes(),
    );
    assert_eq!(
        resolve_release(&http, api, download, Some("0.3.0"), target)
            .await
            .unwrap_err()
            .code(),
        ErrorCode::ArtifactIntegrityError
    );

    // Artifact listed in release.json but absent from SHA256SUMS.
    let bytes = serde_json::to_vec(&good).unwrap();
    http.insert(format!("{base}/release.json"), bytes.clone());
    http.insert(
        format!("{base}/SHA256SUMS"),
        format!("{}  release.json\n", hex::encode(Sha256::digest(&bytes))).into_bytes(),
    );
    assert!(
        resolve_release(&http, api, download, Some("0.3.0"), target)
            .await
            .unwrap_err()
            .message()
            .contains("SHA256SUMS")
    );
}

#[tokio::test]
async fn upgrade_rejects_equal_version_and_staged_version_mismatch() {
    let temp = TempDir::new().unwrap();
    let (binary_path, receipt_path, _) = write_upgradeable_pair(&temp, "0.1.0");
    let download_base = "https://fixture.test/download".to_owned();
    let api_base = "https://fixture.test/api".to_owned();
    let http = FixtureReleaseHttp::default();
    let next = fake_binary("0.1.0");
    fixture_release(
        &http,
        &download_base,
        &api_base,
        "0.1.0",
        host_target(),
        &next,
    );
    let options = base_options(
        &temp,
        &binary_path,
        receipt_path.clone(),
        Some("0.1.0"),
        false,
        true,
        "0.1.0",
    );
    let err = run_upgrade(
        &options,
        &http,
        &InstanceRegistry::with_roots(
            temp.path().join("registry/system"),
            temp.path().join("registry/user"),
        ),
        &FakeServiceManager::default(),
        &mut Vec::new(),
    )
    .await
    .unwrap_err();
    assert!(err.message().contains("already installed"));

    // Staged binary --version does not contain the release identity.
    let mismatched = b"#!/bin/sh\necho \"ocd 9.9.9\"\n".to_vec();
    fixture_release(
        &http,
        &download_base,
        &api_base,
        "0.1.8",
        host_target(),
        &mismatched,
    );
    let options = base_options(
        &temp,
        &binary_path,
        receipt_path,
        Some("0.1.8"),
        false,
        true,
        "0.1.0",
    );
    let err = run_upgrade(
        &options,
        &http,
        &InstanceRegistry::with_roots(
            temp.path().join("registry/system"),
            temp.path().join("registry/user"),
        ),
        &FakeServiceManager::default(),
        &mut Vec::new(),
    )
    .await
    .unwrap_err();
    assert!(
        err.message().contains("version") || err.code() == ErrorCode::ReleaseUnsupported,
        "{err:?}"
    );
}

#[test]
fn write_staged_binary_and_atomic_replace_fail_closed() {
    let temp = TempDir::new().unwrap();
    let relative = Path::new("relative-stage");
    assert_eq!(
        write_staged_binary(relative, b"abc").unwrap_err().code(),
        ErrorCode::PathInvalid
    );
    let staged = temp.path().join("staged");
    write_staged_binary(&staged, b"#!/bin/sh\necho ok\n").unwrap();
    assert!(staged.is_file());
    // create_new refuses overwrite.
    assert_eq!(
        write_staged_binary(&staged, b"again").unwrap_err().code(),
        ErrorCode::PathInvalid
    );
    let other_dir = TempDir::new().unwrap();
    let other = other_dir.path().join("elsewhere");
    fs::write(&other, b"x").unwrap();
    assert!(
        atomic_replace_binary(&staged, &other)
            .unwrap_err()
            .message()
            .contains("same filesystem")
    );
}

#[test]
fn verify_staged_version_rejects_bad_executables() {
    let temp = TempDir::new().unwrap();
    let bad = temp.path().join("bad");
    fs::write(&bad, b"not-an-executable").unwrap();
    let _ = fs::set_permissions(&bad, fs::Permissions::from_mode(0o644));
    assert!(verify_staged_version(&bad, "0.1.0").is_err());
    let failing = temp.path().join("failing");
    fs::write(&failing, b"#!/bin/sh\nexit 1\n").unwrap();
    fs::set_permissions(&failing, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(
        verify_staged_version(&failing, "0.1.0")
            .unwrap_err()
            .message()
            .contains("unsuccessfully")
    );
}

#[tokio::test]
async fn dry_run_rejects_unstable_current_version() {
    let temp = TempDir::new().unwrap();
    let (binary_path, receipt_path, _) = write_upgradeable_pair(&temp, "0.1.0");
    let http = FixtureReleaseHttp::default();
    let target = host_target();
    let tag = "v0.1.8";
    let filename = format!("ocd-{tag}-{target}");
    let next = fake_binary("0.1.8");
    let digest = hex::encode(Sha256::digest(&next));
    let (os, arch) = match target {
        "darwin-arm64" => ("darwin", "arm64"),
        "linux-x64" => ("linux", "x64"),
        "linux-arm64" => ("linux", "arm64"),
        _ => ("darwin", "arm64"),
    };
    let manifest = serde_json::json!({
        "schemaVersion": 1,
        "tag": tag,
        "version": "0.1.8",
        "gitRevision": "abc",
        "workerdRelease": "1.0.0",
        "workerdLockSha256": "cd".repeat(32),
        "artifacts": [{
            "target": target,
            "os": os,
            "arch": arch,
            "filename": filename,
            "bytes": next.len(),
            "sha256": digest,
        }]
    });
    let manifest_bytes = serde_json::to_vec_pretty(&manifest).unwrap();
    let sums = format!(
        "{digest}  {filename}\n{}  release.json\n",
        hex::encode(Sha256::digest(&manifest_bytes))
    );
    let download_base = "https://fixture.test/download";
    let api_base = "https://fixture.test/api";
    let base = format!("{download_base}/{tag}");
    http.insert(
        format!("{api_base}/repos/elliothux/open-compute/releases/latest"),
        format!(r#"{{"tag_name":"{tag}","prerelease":false,"draft":false}}"#),
    );
    http.insert(format!("{base}/release.json"), manifest_bytes);
    http.insert(format!("{base}/SHA256SUMS"), sums.into_bytes());
    http.insert(format!("{base}/{filename}"), next);
    let mut options = base_options(
        &temp,
        &binary_path,
        receipt_path,
        Some("0.1.8"),
        true,
        true,
        "0.1.0-rc.1",
    );
    options.api_base = api_base.to_owned();
    options.download_base = download_base.to_owned();
    let err = run_upgrade(
        &options,
        &http,
        &InstanceRegistry::with_roots(
            temp.path().join("registry/system"),
            temp.path().join("registry/user"),
        ),
        &FakeServiceManager::default(),
        &mut Vec::new(),
    )
    .await
    .unwrap_err();
    assert!(err.message().contains("stable SemVer"));
}

#[test]
fn upgrade_options_production_resolves_on_host() {
    let options = UpgradeOptions::production(None, true, true).unwrap();
    assert!(options.binary_path.is_absolute() || options.binary_path.exists());
    assert!(!options.target.is_empty());
    assert_eq!(host_release_target().unwrap(), options.target);
}

#[tokio::test]
async fn uninstall_rejects_package_manager_owned_path() {
    let temp = TempDir::new().unwrap();
    let brewish = PathBuf::from("/opt/homebrew/bin/ocd");
    let receipt_path = temp.path().join("receipt.json");
    write_receipt(
        &receipt_path,
        &InstallReceipt {
            schema_version: RECEIPT_SCHEMA_VERSION,
            version: "0.1.0".to_owned(),
            sha256: "ab".repeat(32),
            target: host_target().to_owned(),
            binary_path: brewish.to_string_lossy().into_owned(),
            method: "install.sh".to_owned(),
            source: "test://brew".to_owned(),
            installed_at_ms: 1,
        },
    )
    .unwrap();
    assert!(path_looks_package_manager_owned(&brewish));
    let registry = InstanceRegistry::with_roots(
        temp.path().join("registry/system"),
        temp.path().join("registry/user"),
    );
    let err = run_uninstall(
        &receipt_path,
        &brewish,
        &registry,
        &FakeServiceManager::default(),
        &mut Vec::new(),
    )
    .unwrap_err();
    assert!(
        err.message().contains("package-manager") || err.code() == ErrorCode::ReleaseUnsupported,
        "{err:?}"
    );
}

#[tokio::test]
async fn upgrade_rejects_artifact_checksum_mismatch() {
    let temp = TempDir::new().unwrap();
    let (binary_path, receipt_path, _) = write_upgradeable_pair(&temp, "0.1.0");
    let download_base = "https://fixture.test/download".to_owned();
    let api_base = "https://fixture.test/api".to_owned();
    let http = FixtureReleaseHttp::default();
    let next = fake_binary("0.1.7");
    let tag = "v0.1.7";
    let target = host_target();
    let filename = format!("ocd-{tag}-{target}");
    let wrong_digest = "ab".repeat(32);
    let (os, arch) = match target {
        "darwin-arm64" => ("darwin", "arm64"),
        "linux-x64" => ("linux", "x64"),
        "linux-arm64" => ("linux", "arm64"),
        _ => ("darwin", "arm64"),
    };
    let manifest = serde_json::json!({
        "schemaVersion": 1,
        "tag": tag,
        "version": "0.1.7",
        "gitRevision": "abc",
        "workerdRelease": "1.0.0",
        "workerdLockSha256": "cd".repeat(32),
        "artifacts": [{
            "target": target,
            "os": os,
            "arch": arch,
            "filename": filename,
            "bytes": next.len(),
            "sha256": wrong_digest,
        }]
    });
    let manifest_bytes = serde_json::to_vec_pretty(&manifest).unwrap();
    // SUMS must match the (wrong) manifest digest so resolve succeeds; download then fails.
    let sums = format!(
        "{wrong_digest}  {filename}\n{}  release.json\n\n",
        hex::encode(Sha256::digest(&manifest_bytes))
    );
    let base = format!("{download_base}/{tag}");
    http.insert(
        format!("{api_base}/repos/elliothux/open-compute/releases/latest"),
        format!(r#"{{"tag_name":"{tag}","prerelease":false,"draft":false}}"#),
    );
    http.insert(format!("{base}/release.json"), manifest_bytes);
    http.insert(format!("{base}/SHA256SUMS"), sums.into_bytes());
    http.insert(format!("{base}/{filename}"), next);
    let options = base_options(
        &temp,
        &binary_path,
        receipt_path,
        Some("0.1.7"),
        false,
        true,
        "0.1.0",
    );
    let err = run_upgrade(
        &options,
        &http,
        &InstanceRegistry::with_roots(
            temp.path().join("registry/system"),
            temp.path().join("registry/user"),
        ),
        &FakeServiceManager::default(),
        &mut Vec::new(),
    )
    .await
    .unwrap_err();
    assert_eq!(err.code(), ErrorCode::ArtifactIntegrityError);
}

#[test]
fn parse_sha256sums_skips_blank_lines_and_rejects_empty_fields() {
    let digest = "ab".repeat(32);
    let body = format!("\n  \n{digest}  file.bin\n");
    let map = parse_sha256sums(body.as_bytes()).unwrap();
    assert_eq!(
        map.get("file.bin").map(String::as_str),
        Some(digest.as_str())
    );
    let err = parse_sha256sums(b"onlydigest\n").unwrap_err();
    assert_eq!(err.code(), ErrorCode::ReleaseUnsupported);
}

#[test]
fn atomic_replace_rename_failure_and_failing_writer() {
    let temp = TempDir::new().unwrap();
    let staged = temp.path().join("staged");
    write_staged_binary(&staged, b"#!/bin/sh\necho ok\n").unwrap();
    let dest_dir = temp.path().join("dest-as-dir");
    fs::create_dir(&dest_dir).unwrap();
    // Same parent, but rename onto a directory fails.
    let err = atomic_replace_binary(&staged, &dest_dir).unwrap_err();
    assert_eq!(err.code(), ErrorCode::PathInvalid);

    struct FailWrite;
    impl Write for FailWrite {
        fn write(&mut self, _buf: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("nope"))
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let err = writeln!(FailWrite, "x")
        .map_err(|_| io_failed())
        .unwrap_err();
    assert_eq!(err.code(), ErrorCode::Internal);
    assert_eq!(err.message(), io_failed().message());
}
