use super::*;

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
    run_uninstall(
        &receipt_path,
        &binary_path,
        &registry,
        &manager,
        UninstallOptions::default(),
        &mut out,
    )
    .unwrap();
    assert!(String::from_utf8(out).unwrap().contains("UNINSTALL_OK"));
    assert!(!binary_path.exists());
    assert!(!receipt_path.exists());
}
