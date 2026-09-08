use super::*;

pub(super) async fn run() {
    let path = std::env::var_os("OPEN_COMPUTE_TEST_WORKERD")
        .expect("OPEN_COMPUTE_TEST_WORKERD must name the verified stock runtime");
    let path = PathBuf::from(path);
    assert!(
        path.is_absolute(),
        "OPEN_COMPUTE_TEST_WORKERD must be absolute"
    );
    let lock_path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../packages/runtime/workerd.lock.json");
    let lock_path = lock_path.canonicalize().unwrap();
    let assets = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../packages/runtime")
        .canonicalize()
        .unwrap();
    let runtime =
        verify_runtime_binary(&lock_path, &path, Duration::from_secs(10), &Redactor::new())
            .await
            .expect("real workerd must verify");
    let dir = TempDir::new().unwrap();
    let data = dir.path().join("runtime");
    let do_storage = dir.path().join("do-storage");
    fs::create_dir(&data).unwrap();
    fs::create_dir(&do_storage).unwrap();
    let compiler = open_compute_runtime::StaticConfigCompiler::new(
        runtime.clone(),
        lock_path,
        assets,
        data,
        open_compute_runtime::PlatformReleaseMeta {
            version: "0.1.0-test".into(),
        },
        Duration::from_secs(20),
        Redactor::new(),
    );
    let mut cfg = small_cfg();
    cfg.startup_timeout_ms = 20_000;
    cfg.shutdown_grace_ms = 1_000;
    cfg.kill_timeout_ms = 1_000;
    cfg.drain_timeout_ms = 10;
    let running_timeout = Duration::from_millis(cfg.startup_timeout_ms) + Duration::from_secs(5);
    let runtime_source = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let runtime_source_addr = runtime_source.local_addr().unwrap();
    let binding_backend = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let binding_backend_addr = binding_backend.local_addr().unwrap();
    let sup = WorkerdSupervisor::new(
        WorkerdSupervisorOptions {
            runtime,
            compiler,
            config: cfg,
            clock: Arc::new(DeterministicClock::new(UNIX_EPOCH)),
            jitter: Arc::new(SequenceJitter::new(vec![0])),
            redactor: Redactor::new(),
            lease_path: None,
        },
        vec![
            ExternalServiceAddress::loopback("runtime-source", runtime_source_addr).unwrap(),
            ExternalServiceAddress::loopback("binding-backend", binding_backend_addr).unwrap(),
            ExternalServiceAddress::loopback("observability-backend", binding_backend_addr)
                .unwrap(),
        ],
        vec![DirectoryServicePath::local("do-storage", &do_storage).unwrap()],
        Vec::new(),
    );
    sup.start();
    let snap = wait_state_within(&sup, SupervisorState::Running, running_timeout).await;
    let port = snap.listen_port.expect("ephemeral port");
    let pid = snap.pid.unwrap();
    probe_ready_with_raw_token(port, "00".repeat(32).as_str(), Duration::from_secs(2))
        .await
        .expect_err("wrong token must fail against real workerd");
    sup.shutdown().await;
    wait_reaped(pid, Duration::from_secs(5)).unwrap();
    assert_reaped(Some(pid)).unwrap();
}
