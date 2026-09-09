use super::*;

pub(super) async fn run() {
    for point in ["pgid", "stdin", "control", "logs"] {
        let dir = TempDir::new().unwrap();
        let runtime = verified(dir.path()).await;
        let data = dir.path().join("d");
        fs::create_dir(&data).unwrap();
        set_spawn_fail_point(point);
        let mut cfg = small_cfg();
        cfg.restart_budget = 1;
        let sup = WorkerdSupervisor::new(
            WorkerdSupervisorOptions {
                runtime,
                compiler: compiler(data, "ready", None, serde_json::json!({})),
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
        wait_state(&sup, SupervisorState::Failed).await;
        if let Some(pid) = last_spawned_pid() {
            wait_reaped(pid, Duration::from_secs(3)).unwrap();
            assert_reaped(Some(pid)).unwrap();
        } else {
            panic!("expected spawned pid at fail point {point}");
        }
        sup.shutdown().await;
    }
}
