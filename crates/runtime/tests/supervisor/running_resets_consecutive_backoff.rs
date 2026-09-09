use super::*;

pub(super) async fn run() {
    let dir = TempDir::new().unwrap();
    let runtime = verified(dir.path()).await;
    let data = dir.path().join("d");
    fs::create_dir(&data).unwrap();
    let n = Arc::new(std::sync::atomic::AtomicU32::new(0));
    let data2 = data.clone();
    let compiler = FnCompiler(move |token: SecretString, id: StartupId| {
        let data = data2.clone();
        let attempt = n.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Box::pin(async move {
            let mode = if attempt == 0 {
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
    cfg.restart_backoff_initial_ms = 10;
    cfg.restart_backoff_max_ms = 80;
    cfg.restart_budget = 8;
    let clock = Arc::new(DeterministicClock::new(UNIX_EPOCH));
    let sup = WorkerdSupervisor::new(
        WorkerdSupervisorOptions {
            runtime,
            compiler,
            config: cfg,
            clock: clock.clone(),
            jitter: Arc::new(SequenceJitter::new(vec![0, 0, 0, 0])),
            redactor: Redactor::new(),
            lease_path: None,
        },
        Vec::new(),
        Vec::new(),
        Vec::new(),
    );
    sup.start();
    wait_state(&sup, SupervisorState::Running).await;
    let backoff = wait_state(&sup, SupervisorState::BackingOff).await;
    let first = backoff
        .next_retry_at
        .unwrap()
        .duration_since(backoff.last_transition_at)
        .unwrap();
    clock.advance(first + Duration::from_millis(1));
    wait_state(&sup, SupervisorState::Running).await;
    sup.report_unhealthy();
    let backoff2 = wait_state(&sup, SupervisorState::BackingOff).await;
    let second = backoff2
        .next_retry_at
        .unwrap()
        .duration_since(backoff2.last_transition_at)
        .unwrap();
    assert_eq!(
        second, first,
        "successful RUNNING must reset consecutive backoff"
    );
    sup.shutdown().await;
}
