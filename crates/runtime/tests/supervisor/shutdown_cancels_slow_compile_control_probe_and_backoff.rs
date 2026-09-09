use super::*;

pub(super) async fn run() {
    let dir = TempDir::new().unwrap();
    let runtime = verified(dir.path()).await;
    let data = dir.path().join("d");
    fs::create_dir(&data).unwrap();

    let slow = WorkerdSupervisor::new(
        WorkerdSupervisorOptions {
            runtime: runtime.clone(),
            compiler: FnCompiler(|_t, _id| {
                Box::pin(async {
                    tokio::time::sleep(Duration::from_secs(60)).await;
                    Err(open_compute_core::PlatformError::new(
                        ErrorCode::ConfigCompileFailed,
                        "static config compilation failed",
                    ))
                })
                    as Pin<
                        Box<
                            dyn Future<
                                    Output = Result<
                                        CompiledConfig,
                                        open_compute_core::PlatformError,
                                    >,
                                > + Send,
                        >,
                    >
            }),
            config: small_cfg(),
            clock: Arc::new(DeterministicClock::new(UNIX_EPOCH)),
            jitter: Arc::new(SequenceJitter::new(vec![0])),
            redactor: Redactor::new(),
            lease_path: None,
        },
        Vec::new(),
        Vec::new(),
        Vec::new(),
    );
    slow.start();
    tokio::time::sleep(Duration::from_millis(30)).await;
    tokio::time::timeout(Duration::from_secs(2), slow.shutdown())
        .await
        .expect("shutdown during compile");

    for mode in ["no_control", "slow_probe"] {
        let data = dir.path().join(mode);
        fs::create_dir(&data).unwrap();
        let mut cfg = small_cfg();
        cfg.startup_timeout_ms = 30_000;
        let sup = WorkerdSupervisor::new(
            WorkerdSupervisorOptions {
                runtime: runtime.clone(),
                compiler: compiler(data, mode, None, serde_json::json!({})),
                config: cfg,
                clock: Arc::new(DeterministicClock::new(UNIX_EPOCH)),
                jitter: Arc::new(SequenceJitter::new(vec![0])),
                redactor: Redactor::new(),
                lease_path: None,
            },
            Vec::new(),
            Vec::new(),
            Vec::new(),
        );
        sup.start();
        tokio::time::sleep(Duration::from_millis(80)).await;
        let pid = last_spawned_pid();
        tokio::time::timeout(Duration::from_secs(3), sup.shutdown())
            .await
            .unwrap_or_else(|_| panic!("shutdown during {mode}"));
        if let Some(pid) = pid {
            wait_reaped(pid, Duration::from_secs(3)).unwrap();
        }
    }

    let data = dir.path().join("bo");
    fs::create_dir(&data).unwrap();
    let mut cfg = small_cfg();
    cfg.restart_backoff_initial_ms = 60_000;
    cfg.restart_backoff_max_ms = 60_000;
    let clock = Arc::new(DeterministicClock::new(UNIX_EPOCH));
    let sup = WorkerdSupervisor::new(
        WorkerdSupervisorOptions {
            runtime,
            compiler: compiler(data, "early_exit", None, serde_json::json!({})),
            config: cfg,
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
    wait_state(&sup, SupervisorState::BackingOff).await;
    tokio::time::timeout(Duration::from_secs(2), sup.shutdown())
        .await
        .expect("shutdown during backoff");
}
