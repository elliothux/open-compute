use super::*;
use open_compute_core::ErrorCode;
use std::os::unix::fs::symlink;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tempfile::TempDir;

fn sample_receipt(method: &str, binary: &Path) -> InstallReceipt {
    let binary_bytes = fs::read(binary).unwrap_or_default();
    InstallReceipt {
        schema_version: RECEIPT_SCHEMA_VERSION,
        version: "0.1.0".to_owned(),
        sha256: hex::encode(Sha256::digest(binary_bytes)),
        target: "darwin-arm64".to_owned(),
        binary_path: binary.to_string_lossy().into_owned(),
        method: method.to_owned(),
        source: "test://fixture".to_owned(),
        installed_at_ms: 1,
    }
}

#[test]
fn write_receipt_refuses_package_manager_overwrite() {
    let temp = TempDir::new().unwrap();
    let binary = temp.path().join("bin/ocd");
    fs::create_dir_all(binary.parent().unwrap()).unwrap();
    fs::write(&binary, b"ocd").unwrap();
    let receipt_path = temp.path().join("share/open-compute/install-receipt.json");
    write_receipt(&receipt_path, &sample_receipt("homebrew", &binary)).unwrap();
    let err = write_receipt(&receipt_path, &sample_receipt("install.sh", &binary)).unwrap_err();
    assert_eq!(err.code(), ErrorCode::PathInvalid);
    assert!(err.message().contains("package-manager-owned"));
}

#[test]
fn require_upgradeable_rejects_package_manager_method() {
    let temp = TempDir::new().unwrap();
    let binary = temp.path().join("bin/ocd");
    fs::create_dir_all(binary.parent().unwrap()).unwrap();
    fs::write(&binary, b"ocd").unwrap();
    let receipt_path = receipt_path_for_binary(&binary);
    fs::create_dir_all(receipt_path.parent().unwrap()).unwrap();
    write_receipt(&receipt_path, &sample_receipt("apt", &binary)).unwrap();
    let err = require_upgradeable_receipt(&receipt_path, &binary).unwrap_err();
    assert_eq!(err.code(), ErrorCode::ReleaseUnsupported);
}

#[test]
fn require_upgradeable_rejects_cellar_path() {
    let path = Path::new("/opt/homebrew/Cellar/open-compute/0.1.0/bin/ocd");
    let err =
        require_upgradeable_receipt(Path::new("/tmp/missing-receipt.json"), path).unwrap_err();
    assert_eq!(err.code(), ErrorCode::ReleaseUnsupported);
    assert!(err.message().contains("package-manager-owned"));
}

#[test]
fn stable_semver_helpers() {
    assert!(is_stable_semver("0.1.0"));
    assert!(!is_stable_semver("0.1.0-rc.1"));
    assert!(!is_stable_semver("v0.1.0"));
    assert_eq!(
        cmp_stable_semver("0.1.1", "0.1.0"),
        Some(std::cmp::Ordering::Greater)
    );
    assert_eq!(
        unix_ms_now(UNIX_EPOCH + Duration::from_secs(1)).unwrap(),
        1000
    );
    let _ = SystemTime::now();
}

#[test]
fn validate_rejects_corrupt_fields() {
    let binary = Path::new("/usr/local/bin/ocd");
    let mut receipt = sample_receipt("manual", binary);
    receipt.schema_version = 99;
    assert!(receipt.validate().is_err());
    receipt = sample_receipt("manual", binary);
    receipt.version = "v0.1.0".to_owned();
    assert!(receipt.validate().is_err());
    receipt = sample_receipt("manual", binary);
    receipt.sha256 = "zz".to_owned();
    assert!(receipt.validate().is_err());
    receipt = sample_receipt("manual", binary);
    receipt.target.clear();
    assert!(receipt.validate().is_err());
    receipt = sample_receipt("manual", binary);
    receipt.binary_path = "relative".to_owned();
    assert!(receipt.validate().is_err());
}

#[test]
fn read_receipt_rejects_symlink_and_oversize() {
    let temp = TempDir::new().unwrap();
    let target = temp.path().join("target.json");
    fs::write(&target, b"{}").unwrap();
    let link = temp.path().join("link.json");
    symlink(&target, &link).unwrap();
    let err = read_receipt(&link).unwrap_err();
    assert!(err.message().contains("symlink"));

    let big = temp.path().join("big.json");
    fs::write(&big, vec![b'a'; 17 * 1024]).unwrap();
    let err = read_receipt(&big).unwrap_err();
    assert!(err.message().contains("size bound"));
}

#[test]
fn write_receipt_refuses_unreadable_existing() {
    let temp = TempDir::new().unwrap();
    let binary = temp.path().join("bin/ocd");
    fs::create_dir_all(binary.parent().unwrap()).unwrap();
    fs::write(&binary, b"ocd").unwrap();
    let receipt_path = temp.path().join("share/open-compute/install-receipt.json");
    fs::create_dir_all(receipt_path.parent().unwrap()).unwrap();
    fs::write(&receipt_path, b"{nope").unwrap();
    let err = write_receipt(&receipt_path, &sample_receipt("install.sh", &binary)).unwrap_err();
    assert!(err.message().contains("unreadable"));
}

#[test]
fn remove_receipt_is_idempotent() {
    let temp = TempDir::new().unwrap();
    let binary = temp.path().join("bin/ocd");
    fs::create_dir_all(binary.parent().unwrap()).unwrap();
    fs::write(&binary, b"ocd").unwrap();
    let receipt_path = temp.path().join("share/open-compute/install-receipt.json");
    write_receipt(&receipt_path, &sample_receipt("manual", &binary)).unwrap();
    remove_receipt(&receipt_path).unwrap();
    remove_receipt(&receipt_path).unwrap();
    assert!(!receipt_path.exists());
}

#[test]
fn require_upgradeable_rejects_binary_mismatch() {
    let temp = TempDir::new().unwrap();
    let binary = temp.path().join("bin/ocd");
    let other = temp.path().join("bin/other");
    fs::create_dir_all(binary.parent().unwrap()).unwrap();
    fs::write(&binary, b"ocd").unwrap();
    fs::write(&other, b"ocd").unwrap();
    let receipt_path = receipt_path_for_binary(&binary);
    fs::create_dir_all(receipt_path.parent().unwrap()).unwrap();
    write_receipt(&receipt_path, &sample_receipt("manual", &binary)).unwrap();
    let err = require_upgradeable_receipt(&receipt_path, &other).unwrap_err();
    assert!(err.message().contains("does not match"));
}

#[test]
fn path_looks_package_manager_owned_variants() {
    assert!(path_looks_package_manager_owned(Path::new(
        "/home/linuxbrew/.linuxbrew/bin/ocd"
    )));
    assert!(path_looks_package_manager_owned(Path::new("/usr/bin/ocd")));
    assert!(!path_looks_package_manager_owned(Path::new(
        "/usr/local/bin/ocd"
    )));
}

#[test]
fn receipt_path_and_production_helpers() {
    assert!(
        receipt_path_for_binary(Path::new("/usr/local/bin/ocd"))
            .ends_with("share/open-compute/install-receipt.json")
    );
    assert_eq!(
        receipt_path_for_binary(Path::new("ocd")),
        PathBuf::from(DEFAULT_RECEIPT_PATH)
    );
    let _ = production_receipt_path();
}

#[test]
fn write_receipt_rejects_non_directory_parent_and_round_trips_manual() {
    let temp = TempDir::new().unwrap();
    let binary = temp.path().join("bin/ocd");
    fs::create_dir_all(binary.parent().unwrap()).unwrap();
    fs::write(&binary, b"ocd").unwrap();
    let parent = temp.path().join("not-a-dir");
    fs::write(&parent, b"x").unwrap();
    let err = write_receipt(
        &parent.join("install-receipt.json"),
        &sample_receipt("manual", &binary),
    )
    .unwrap_err();
    assert_eq!(err.code(), ErrorCode::PathInvalid);

    let ok_path = temp.path().join("share/open-compute/install-receipt.json");
    write_receipt(&ok_path, &sample_receipt("install.sh", &binary)).unwrap();
    let loaded = read_receipt(&ok_path).unwrap();
    assert!(loaded.is_self_managed());
    assert_eq!(loaded.method, "install.sh");
    require_upgradeable_receipt(&ok_path, &binary).unwrap();
}

#[test]
fn validate_rejects_empty_source_and_method() {
    let binary = Path::new("/usr/local/bin/ocd");
    let mut receipt = sample_receipt("manual", binary);
    receipt.source.clear();
    assert!(receipt.validate().is_err());
    receipt = sample_receipt("manual", binary);
    receipt.method.clear();
    assert!(receipt.validate().is_err());
    assert!(!is_stable_semver(""));
    assert!(!is_stable_semver("01.0.0"));
    assert!(!is_stable_semver("0.1.0.1"));
    assert_eq!(cmp_stable_semver("0.1.0", "0.1.0-rc.1"), None);
    assert_eq!(parse_stable_semver("0.10.2"), Some((0, 10, 2)));
    assert_eq!(parse_stable_semver("0.1.0.9"), None);
}

#[test]
fn read_receipt_rejects_directory_and_relative_path() {
    let temp = TempDir::new().unwrap();
    let dir = temp.path().join("receipt-dir");
    fs::create_dir_all(&dir).unwrap();
    let err = read_receipt(&dir).unwrap_err();
    assert!(err.message().contains("regular file"));
    let err = read_receipt(Path::new("relative-receipt.json")).unwrap_err();
    assert_eq!(err.code(), ErrorCode::PathInvalid);
    assert!(err.message().contains("absolute"));
}

#[test]
fn write_receipt_allows_self_managed_method_swap_and_blocks_pm() {
    let temp = TempDir::new().unwrap();
    let binary = temp.path().join("bin/ocd");
    fs::create_dir_all(binary.parent().unwrap()).unwrap();
    fs::write(&binary, b"ocd").unwrap();
    let path = temp.path().join("share/open-compute/install-receipt.json");
    write_receipt(&path, &sample_receipt("install.sh", &binary)).unwrap();
    write_receipt(&path, &sample_receipt("manual", &binary)).unwrap();
    let loaded = read_receipt(&path).unwrap();
    assert_eq!(loaded.method, "manual");

    let pm_path = temp.path().join("share/open-compute/pm-receipt.json");
    write_receipt(&pm_path, &sample_receipt("homebrew", &binary)).unwrap();
    // Same package-manager method still refuses overwrite of a non-self-managed receipt.
    let err = write_receipt(&pm_path, &sample_receipt("homebrew", &binary)).unwrap_err();
    assert!(err.message().contains("package-manager-owned"));
}

#[test]
fn remove_receipt_fails_when_path_is_directory() {
    let temp = TempDir::new().unwrap();
    let dir = temp.path().join("not-a-receipt");
    fs::create_dir_all(&dir).unwrap();
    let err = remove_receipt(&dir).unwrap_err();
    assert_eq!(err.code(), ErrorCode::PathInvalid);
}

#[test]
fn unix_ms_rejects_pre_epoch_and_parent_symlink() {
    let err = unix_ms_now(UNIX_EPOCH - Duration::from_secs(1)).unwrap_err();
    assert_eq!(err.code(), ErrorCode::Internal);

    let temp = TempDir::new().unwrap();
    let real = temp.path().join("real-parent");
    fs::create_dir_all(&real).unwrap();
    let link = temp.path().join("link-parent");
    symlink(&real, &link).unwrap();
    let binary = temp.path().join("bin/ocd");
    fs::create_dir_all(binary.parent().unwrap()).unwrap();
    fs::write(&binary, b"ocd").unwrap();
    let err = write_receipt(
        &link.join("install-receipt.json"),
        &sample_receipt("manual", &binary),
    )
    .unwrap_err();
    assert_eq!(err.code(), ErrorCode::PathInvalid);
}

#[test]
fn read_receipt_fails_when_bytes_unreadable() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("install-receipt.json");
    fs::write(&path, br#"{"schema_version":1}"#).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o000)).unwrap();
    let err = read_receipt(&path);
    let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o600));
    assert!(err.is_err());
}

#[test]
fn production_receipt_path_resolves() {
    let path = production_receipt_path().unwrap();
    assert!(path.is_absolute());
    assert!(path.ends_with("install-receipt.json") || path == Path::new(DEFAULT_RECEIPT_PATH));
}
