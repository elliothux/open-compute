use super::*;

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
        UninstallOptions::default(),
        &mut Vec::new(),
    )
    .unwrap_err();
    assert!(
        err.message().contains("does not match") || err.code() == ErrorCode::ReleaseUnsupported
    );
    let _ = binary_path;
}
