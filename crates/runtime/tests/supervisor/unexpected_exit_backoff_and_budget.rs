use super::*;

pub(super) async fn run() {
    let dir = TempDir::new().unwrap();
    let runtime = verified(dir.path()).await;
    let data = dir.path().join("d");
    fs::create_dir(&data).unwrap();
    let mut cfg = small_cfg();
    cfg.restart_budget = 3;
    cfg.restart_backoff_initial_ms = 5;
    cfg.restart_backoff_max_ms = 40;
    let clock = Arc::new(DeterministicClock::new(UNIX_EPOCH));
    let sup = WorkerdSupervisor::new(
        WorkerdSupervisorOptions {
            runtime,
            compiler: compiler(data, "early_exit", None, serde_json::json!({})),
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
    let mut fingerprints = Vec::new();
    let mut seen_backoff = 0u32;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        clock.advance(Duration::from_millis(50));
        let snap = sup.snapshot();
        if let Some(fp) = snap.token_fingerprint.clone()
            && !fingerprints.contains(&fp)
        {
            fingerprints.push(fp);
        }
        if snap.state == SupervisorState::BackingOff {
            seen_backoff += 1;
        }
        if snap.state == SupervisorState::Failed {
            break;
        }
        if tokio::time::Instant::now() > deadline {
            panic!("did not fail, last={snap:?}");
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(
        fingerprints.len() >= 3,
        "fresh token each attempt {fingerprints:?}"
    );
    assert!(seen_backoff >= 1);
    let final_snap = sup.snapshot();
    assert_eq!(final_snap.reason, ReadinessReason::RuntimeInvalid);
    let _ = seen_backoff;
    sup.shutdown().await;
}
