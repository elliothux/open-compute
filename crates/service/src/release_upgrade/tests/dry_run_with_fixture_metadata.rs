use super::*;

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
