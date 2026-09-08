use super::*;

pub(super) async fn run() {
    for mode in [
        "malformed_control",
        "oversized_control",
        "duplicate_control",
        "wrong_socket",
        "non_loopback",
        "timeout",
        "early_exit",
        "bind_fail",
        "no_control",
    ] {
        let dir = TempDir::new().unwrap();
        let runtime = verified(dir.path()).await;
        let data = dir.path().join("d");
        fs::create_dir(&data).unwrap();
        let mut cfg = small_cfg();
        cfg.startup_timeout_ms = 300;
        cfg.restart_budget = 1;
        let sup = WorkerdSupervisor::new(
            WorkerdSupervisorOptions {
                runtime,
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
        let snap = wait_state(&sup, SupervisorState::Failed).await;
        if let Some(pid) = snap.pid {
            assert!(!pid_alive(pid));
        }
        sup.shutdown().await;
        let _ = mode;
    }
}
