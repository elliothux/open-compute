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
    let dir = TempDir::new_in("/tmp").unwrap();
    let data = dir.path().join("runtime");
    let do_storage = dir.path().join("do-storage");
    fs::create_dir(&data).unwrap();
    fs::create_dir(&do_storage).unwrap();
    let compiler = open_compute_runtime::StaticConfigCompiler::new(
        runtime.clone(),
        lock_path,
        assets,
        data.clone(),
        data.join("ingress.sock"),
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
    let runtime_source_addr = dir.path().join("source.sock");
    let binding_backend_addr = dir.path().join("bindings.sock");
    let _runtime_source = tokio::net::UnixListener::bind(&runtime_source_addr).unwrap();
    let _binding_backend = tokio::net::UnixListener::bind(&binding_backend_addr).unwrap();
    let sup = WorkerdSupervisor::new(
        WorkerdSupervisorOptions {
            runtime,
            compiler,
            config: cfg,
            clock: Arc::new(DeterministicClock::new(UNIX_EPOCH)),
            jitter: Arc::new(SequenceJitter::new(vec![0])),
            lease_path: None,
        },
        vec![
            ExternalServiceAddress::unix("runtime-source", &runtime_source_addr).unwrap(),
            ExternalServiceAddress::unix("binding-backend", &binding_backend_addr).unwrap(),
            ExternalServiceAddress::unix("observability-backend", &binding_backend_addr).unwrap(),
        ],
        vec![DirectoryServicePath::local("do-storage", &do_storage).unwrap()],
    );
    sup.start();
    let snap = wait_state_within(&sup, SupervisorState::Running, running_timeout).await;
    let socket = snap.ingress_socket.as_ref().expect("private Unix socket");
    let pid = snap.pid.unwrap();
    probe_ready(socket, Duration::from_secs(2)).await.unwrap();
    sup.shutdown().await;
    wait_reaped(pid, Duration::from_secs(5)).unwrap();
    assert_reaped(Some(pid)).unwrap();
}
