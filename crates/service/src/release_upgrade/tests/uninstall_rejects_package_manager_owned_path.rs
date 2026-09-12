use super::*;

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
        UninstallOptions::default(),
        &mut Vec::new(),
    )
    .unwrap_err();
    assert!(
        err.message().contains("package-manager") || err.code() == ErrorCode::ReleaseUnsupported,
        "{err:?}"
    );
}
