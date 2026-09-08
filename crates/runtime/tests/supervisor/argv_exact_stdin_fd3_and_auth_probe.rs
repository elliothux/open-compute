use super::*;

pub(super) async fn run() {
    let dir = TempDir::new().unwrap();
    let runtime = verified(dir.path()).await;
    let argv_path = dir.path().join("argv.json");
    let stdin_path = dir.path().join("stdin.bin");
    let data = dir.path().join("runtime-data");
    fs::create_dir(&data).unwrap();
    let clock = Arc::new(DeterministicClock::new(UNIX_EPOCH));
    let extra = serde_json::json!({"stdin_marker_path": stdin_path.display().to_string()});
    let sup = WorkerdSupervisor::new(
        WorkerdSupervisorOptions {
            runtime: runtime.clone(),
            compiler: compiler(data, "ready", Some(argv_path.clone()), extra),
            config: small_cfg(),
            clock,
            jitter: Arc::new(SequenceJitter::new(vec![0])),
            redactor: Redactor::new(),
            lease_path: None,
        },
        Vec::new(),
        Vec::new(),
        Vec::new(),
    );
    sup.start();
    let snap = wait_state(&sup, SupervisorState::Running).await;
    assert_eq!(snap.reason, ReadinessReason::Ready);
    assert_eq!(snap.pid, snap.pgid);
    let argv: Vec<String> = serde_json::from_slice(&fs::read(&argv_path).unwrap()).unwrap();
    assert_eq!(argv, serve_argv(runtime.lock()));
    let joined = argv.join(" ");
    assert!(!joined.contains("token"));
    let stdin = fs::read(&stdin_path).unwrap();
    assert!(stdin.windows(5).any(|w| w == b"token"));
    let port = snap.listen_port.expect("port");
    probe_ready_with_raw_token(port, "00".repeat(32).as_str(), Duration::from_secs(1))
        .await
        .expect_err("wrong token must fail closed");
    let fp = snap.token_fingerprint.clone().unwrap();
    assert_eq!(fp.len(), 16);
    sup.shutdown().await;
    wait_reaped(snap.pid.unwrap(), Duration::from_secs(2)).unwrap();
}
