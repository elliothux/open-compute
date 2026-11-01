use super::*;

pub(super) async fn run() {
    let dir = TempDir::new_in("/tmp").unwrap();
    let runtime = verified(dir.path()).await;
    let data = dir.path().join("d");
    fs::create_dir(&data).unwrap();
    let n = Arc::new(std::sync::atomic::AtomicU32::new(0));
    let data2 = data.clone();
    let n_spawn = n.clone();
    let compiler = FnCompiler(move |id: StartupId| {
        let data = data2.clone();
        let attempt = n_spawn.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Box::pin(async move {
            if attempt == 0 {
                let body = serde_json::json!({"mode": "crash_after_ready", "socket": data.join("ingress.sock")});
                let digest = id.to_string().replace('-', "");
                CompiledConfig::from_bytes_for_test(
                    &data,
                    &digest,
                    &serde_json::to_vec(&body).unwrap(),
                )
            } else {
                Err(open_compute_core::PlatformError::new(
                    ErrorCode::ConfigCompileFailed,
                    "static config compilation failed",
                ))
            }
        })
            as Pin<
                Box<
                    dyn Future<Output = Result<CompiledConfig, open_compute_core::PlatformError>>
                        + Send,
                >,
            >
    });
    let mut cfg = small_cfg();
    cfg.restart_budget = 8;
    cfg.restart_backoff_initial_ms = 5;
    cfg.restart_backoff_max_ms = 10;
    let clock = Arc::new(DeterministicClock::new(UNIX_EPOCH));
    let sup = WorkerdSupervisor::new(
        WorkerdSupervisorOptions {
            runtime,
            compiler,
            config: cfg,
            clock: clock.clone(),
            jitter: Arc::new(SequenceJitter::new(vec![0, 0, 0, 0])),
            lease_path: None,
        },
        Vec::new(),
        Vec::new(),
    );
    sup.start();
    wait_state(&sup, SupervisorState::Running).await;
    let backoff = wait_state(&sup, SupervisorState::BackingOff).await;
    let gen1_startup_id = backoff.startup_id.expect("generation 1 startup id");
    assert_eq!(backoff.last_exit_startup_id, Some(gen1_startup_id));
    let gen1 = backoff.last_exit.clone().expect("generation 1 exit");
    assert_eq!(gen1.code, Some(9));
    clock.advance(Duration::from_millis(50));
    let snap = wait_state(&sup, SupervisorState::Failed).await;
    assert_eq!(snap.last_exit_startup_id, snap.startup_id);
    let exit = snap.last_exit.expect("generation 2 exit");
    assert_eq!(exit.code_name, "CONFIG_COMPILE_FAILED");
    assert_eq!(
        exit.code, None,
        "compile failure must not inherit prior exit code"
    );
    assert_eq!(
        exit.signal, None,
        "compile failure must not inherit prior signal"
    );
    sup.shutdown().await;
}
