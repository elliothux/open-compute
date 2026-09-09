use super::*;

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
