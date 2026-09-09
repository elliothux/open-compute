use super::*;

pub(super) async fn run() {
    let dir = TempDir::new().unwrap();
    let runtime = verified(dir.path()).await;
    let data = dir.path().join("d");
    fs::create_dir(&data).unwrap();
    let n = Arc::new(std::sync::atomic::AtomicU32::new(0));
    let data2 = data.clone();
    let n_spawn = n.clone();
    let compiler = FnCompiler(move |token: SecretString, id: StartupId| {
        let data = data2.clone();
        let attempt = n_spawn.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Box::pin(async move {
            let mode = if attempt < 4 {
                "crash_after_ready"
            } else {
                "ready"
            };
            let body = serde_json::json!({"mode": mode, "token": token.expose()});
            let digest = id.to_string().replace('-', "");
            CompiledConfig::from_bytes_for_test(&data, &digest, &serde_json::to_vec(&body).unwrap())
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
            jitter: Arc::new(SequenceJitter::new(vec![0, 0, 0, 0, 0, 0, 0, 0])),
            redactor: Redactor::new(),
            lease_path: None,
        },
        Vec::new(),
        Vec::new(),
        Vec::new(),
    );
    sup.start();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    loop {
        clock.advance(Duration::from_millis(20));
        if sup.snapshot().state == SupervisorState::Running
            && n.load(std::sync::atomic::Ordering::SeqCst) >= 5
        {
            break;
        }
        if tokio::time::Instant::now() > deadline {
            panic!(
                "did not reach later running generation, last={:?}",
                sup.snapshot()
            );
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    wait_state(&sup, SupervisorState::Running).await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(
        sup.owner_registry_len() <= 1,
        "registry leaked senders: {}",
        sup.owner_registry_len()
    );
    sup.shutdown().await;
    assert_eq!(sup.owner_registry_len(), 0);
}
