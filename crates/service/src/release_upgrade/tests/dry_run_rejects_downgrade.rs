use super::*;

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
