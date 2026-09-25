use super::*;

#[tokio::test]
async fn upgrade_restart_failure_stops_remaining() {
    let temp = TempDir::new().unwrap();
    let (binary_path, receipt_path, current) = write_upgradeable_pair(&temp, "0.1.0");
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
    registry
        .register(
            &config.canonicalize().unwrap(),
            ServiceScope::User,
            SystemTime::now(),
        )
        .unwrap();
    let manager = FakeServiceManager::default();
    manager
        .install(ServiceScope::User, None, &binary_path)
        .unwrap();
    manager.start(ServiceScope::User).unwrap();
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
    assert!(!String::from_utf8(out).unwrap().contains("UPGRADE_OK"));
    assert_eq!(fs::read(&binary_path).unwrap(), current);
    assert!(backup_path(&binary_path).unwrap().is_file());
    assert!(backup_path(&options.receipt_path).unwrap().is_file());

    manager.set_fail_restart(false);
    let mut restore_out = Vec::new();
    run_upgrade_restore(&options, &registry, &manager, &mut restore_out).unwrap();
    assert!(
        String::from_utf8(restore_out)
            .unwrap()
            .contains("UPGRADE_RESTORE_OK 0.1.0")
    );
    assert_eq!(fs::read(&binary_path).unwrap(), current);
    assert!(!backup_path(&binary_path).unwrap().exists());
    assert!(!backup_path(&options.receipt_path).unwrap().exists());
    assert_eq!(
        run_upgrade_restore(&options, &registry, &manager, &mut Vec::new())
            .unwrap_err()
            .code(),
        ErrorCode::ReleaseUnsupported
    );

    let backups = UpgradeBackups::create(&options).unwrap();
    fs::remove_file(&backups.binary).unwrap();
    fs::write(&backups.binary, b"corrupt backup").unwrap();
    assert_eq!(
        run_upgrade_restore(&options, &registry, &manager, &mut Vec::new())
            .unwrap_err()
            .code(),
        ErrorCode::ArtifactIntegrityError
    );
    assert_eq!(
        backups.ensure_absent().unwrap_err().code(),
        ErrorCode::ReleaseUnsupported
    );
    fs::remove_file(&backups.binary).unwrap();
    fs::remove_file(&backups.receipt).unwrap();
    assert_eq!(backups.remove().unwrap_err().code(), ErrorCode::PathInvalid);

    let mut missing_binary = options.clone();
    missing_binary.binary_path = temp.path().join("missing-binary");
    assert_eq!(
        UpgradeBackups::create(&missing_binary)
            .err()
            .unwrap()
            .code(),
        ErrorCode::PathInvalid
    );
    let mut missing_receipt = options.clone();
    missing_receipt.receipt_path = temp.path().join("missing-receipt");
    assert_eq!(
        UpgradeBackups::create(&missing_receipt)
            .err()
            .unwrap()
            .code(),
        ErrorCode::PathInvalid
    );
    assert!(!backup_path(&missing_receipt.binary_path).unwrap().exists());

    use std::os::unix::ffi::OsStringExt as _;
    assert_eq!(
        backup_path(Path::new(&std::ffi::OsString::from_vec(vec![0xff])))
            .unwrap_err()
            .code(),
        ErrorCode::PathInvalid
    );
}
