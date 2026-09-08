use super::*;

pub(super) async fn run() {
    for mode in ["late_duplicate_control", "late_malformed_control"] {
        let dir = TempDir::new().unwrap();
        let runtime = verified(dir.path()).await;
        let data = dir.path().join("d");
        fs::create_dir(&data).unwrap();
        let mut cfg = small_cfg();
        cfg.restart_budget = 3;
        let clock = Arc::new(DeterministicClock::new(UNIX_EPOCH));
        let sup = WorkerdSupervisor::new(
            WorkerdSupervisorOptions {
                runtime,
                compiler: compiler(data, mode, None, serde_json::json!({})),
                config: cfg,
                clock: clock.clone(),
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
        let pid = snap.pid.unwrap();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        loop {
            clock.advance(Duration::from_millis(20));
            let now = sup.snapshot();
            if now.state == SupervisorState::BackingOff || now.state == SupervisorState::Starting {
                break;
            }
            if now.state == SupervisorState::Failed {
                break;
            }
            if tokio::time::Instant::now() > deadline {
                panic!("{mode} did not teardown, last={now:?}");
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        wait_reaped(pid, Duration::from_secs(3)).unwrap();
        sup.shutdown().await;
    }
}
