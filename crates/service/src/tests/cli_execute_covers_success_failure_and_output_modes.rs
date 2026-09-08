use super::*;

#[tokio::test]
async fn cli_execute_covers_success_failure_and_output_modes() {
    let dir = TempDir::new().unwrap();
    let path = write_config(dir.path(), "");
    assert!(load_checked(&path).is_ok());

    for json in [false, true] {
        let mut args = vec![
            "ocd".to_owned(),
            "--config".to_owned(),
            path.display().to_string(),
            "config".to_owned(),
            "check".to_owned(),
        ];
        if json {
            args.push("--json".to_owned());
        }
        let cli = parse_from(args).unwrap();
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let code = execute(cli, &mut stdout, &mut stderr).await;
        assert_eq!(code, std::process::ExitCode::SUCCESS);
        assert!(stderr.is_empty());
        let text = String::from_utf8(stdout).unwrap();
        assert!(text.contains(if json { "config_check" } else { "CONFIG_OK" }));
    }

    let loaded = load_fixture_platform_config(&path);
    let storage = open_compute_storage::PlatformStorage::bootstrap(
        &loaded.config.data,
        &open_compute_core::SystemClock,
    )
    .unwrap();
    let connected =
        crate::object_storage::connect_object_backend(&loaded.config, storage.identity()).unwrap();
    storage
        .bind_object_authority(
            connected.backend.kind(),
            &connected.backend.authority_sha256(),
        )
        .unwrap();
    open_compute_artifacts::preflight_object_storage(
        &connected.backend,
        storage.identity().platform_id,
        open_compute_core::StartupId::generate(),
    )
    .await
    .unwrap();
    let scheduler_path = storage.data_dir().ensure_scheduler_db().unwrap();
    fs::write(&scheduler_path, b"corrupt scheduler").unwrap();
    drop(storage);
    let recovery = parse_from([
        "ocd",
        "--config",
        path.to_str().unwrap(),
        "scheduler",
        "recover-corrupt",
        "--backup-name",
        "scheduler-corrupt-cli-test",
    ])
    .unwrap();
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    assert_eq!(
        execute(recovery, &mut stdout, &mut stderr).await,
        std::process::ExitCode::SUCCESS
    );
    assert!(stderr.is_empty());
    assert!(
        String::from_utf8(stdout)
            .unwrap()
            .contains("SCHEDULER_RECOVERED")
    );
    assert!(inspect_scheduler_db(&scheduler_path, 100, 10).is_ok());
    assert_eq!(
        fs::read(
            loaded
                .config
                .data
                .path
                .join("diagnostics/scheduler-recovery/scheduler-corrupt-cli-test/scheduler.sqlite")
        )
        .unwrap(),
        b"corrupt scheduler"
    );

    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let missing_config = parse_from(["ocd", "config", "check"]).unwrap();
    let code = execute(missing_config, &mut stdout, &mut stderr).await;
    assert_ne!(code, std::process::ExitCode::SUCCESS);
    assert!(
        String::from_utf8(stderr)
            .unwrap()
            .contains("CONFIG_PATH_INVALID")
    );

    assert!(parse_from(["ocd", "package-release"]).is_err());

    for json in [false, true] {
        let mut args = vec![
            "ocd".to_owned(),
            "--config".to_owned(),
            path.display().to_string(),
            "doctor".to_owned(),
        ];
        if json {
            args.push("--json".to_owned());
        }
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let code = execute(parse_from(args).unwrap(), &mut stdout, &mut stderr).await;
        assert_eq!(code, std::process::ExitCode::from(ExitClass::Doctor.code()));
        assert!(stderr.is_empty());
        let output = String::from_utf8(stdout).unwrap();
        assert!(output.contains(if json {
            "\"command\":\"doctor\""
        } else {
            "DOCTOR"
        }));
    }

    struct RejectWrites;
    impl Write for RejectWrites {
        fn write(&mut self, _buf: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("rejected"))
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let cli = parse_from(["ocd", "--config", path.to_str().unwrap(), "config", "check"]).unwrap();
    let mut stdout = RejectWrites;
    let mut stderr = Vec::new();
    assert_ne!(
        execute(cli, &mut stdout, &mut stderr).await,
        std::process::ExitCode::SUCCESS
    );
    assert!(
        String::from_utf8(stderr)
            .unwrap()
            .contains("CONFIG_INVALID")
    );
}
