use super::*;

#[test]
fn digest_assets_and_unix_addresses_are_fail_closed() {
    assert_eq!(
        load_assets(Path::new("relative-assets"))
            .unwrap_err()
            .code(),
        ErrorCode::PathInvalid
    );
    let dir = TempDir::new().unwrap();
    fs::create_dir(dir.path().join("dist")).unwrap();
    fs::write(dir.path().join("config.capnp"), b"template").unwrap();
    assert_eq!(
        load_assets(dir.path()).unwrap_err().code(),
        ErrorCode::ConfigCompileFailed
    );
    fs::write(dir.path().join("dist/worker.js"), b"export default {}").unwrap();
    fs::create_dir_all(dir.path().join("dist/ai")).unwrap();
    fs::create_dir_all(dir.path().join("dist/ai-search")).unwrap();
    fs::write(dir.path().join("dist/ai/index.js"), b"export default {}").unwrap();
    fs::write(
        dir.path().join("dist/ai-search/index.js"),
        b"export default {}",
    )
    .unwrap();
    let (_, workers, config_path) = load_assets(dir.path()).unwrap();
    assert_eq!(
        workers
            .iter()
            .map(|(path, _)| path.as_str())
            .collect::<Vec<_>>(),
        [
            "dist/ai-search/index.js",
            "dist/ai/index.js",
            "dist/worker.js"
        ]
    );
    assert_eq!(config_path, dir.path().join("config.capnp"));

    use crate::supervisor::{ExternalServiceAddress, SupervisorSnapshot, SupervisorState};
    let template = "const address :Text = \"__OPEN_COMPUTE_INGRESS_ADDRESS__\";";
    assert_eq!(
        render_config(template, Path::new("/tmp/private.sock")).unwrap(),
        "const address :Text = \"unix:/tmp/private.sock\";"
    );
    for path in [
        "relative.sock".to_owned(),
        format!("/{}", "x".repeat(103)),
        "/tmp/bad\0sock".to_owned(),
    ] {
        assert!(render_config(template, Path::new(&path)).is_err());
        assert!(ExternalServiceAddress::unix("runtime-source", Path::new(&path)).is_err());
    }
    assert!(render_config("missing", Path::new("/tmp/a.sock")).is_err());
    assert!(ExternalServiceAddress::unix("runtime-source", Path::new("/tmp/a.sock")).is_ok());
    for name in ["", &"x".repeat(65), "bad name"] {
        assert!(ExternalServiceAddress::unix(name, Path::new("/tmp/a.sock")).is_err());
    }
    let states = [
        (SupervisorState::Stopped, "STOPPED"),
        (SupervisorState::Starting, "STARTING"),
        (SupervisorState::Running, "RUNNING"),
        (SupervisorState::BackingOff, "BACKING_OFF"),
        (SupervisorState::Failed, "FAILED"),
        (SupervisorState::Draining, "DRAINING"),
        (SupervisorState::Stopping, "STOPPING"),
    ];
    for (state, expected) in states {
        assert_eq!(state.as_str(), expected);
    }
    let snapshot = SupervisorSnapshot::initial(SystemTime::UNIX_EPOCH, "digest".into());
    assert_eq!(snapshot.state, SupervisorState::Stopped);
    assert_eq!(snapshot.reason, ReadinessReason::Starting);
    assert!(!format!("{snapshot:?}").contains("ingress_socket"));
}
