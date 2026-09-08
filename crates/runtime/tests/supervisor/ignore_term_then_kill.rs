use super::*;

pub(super) async fn run() {
    let dir = TempDir::new().unwrap();
    let runtime = verified(dir.path()).await;
    let data = dir.path().join("d");
    fs::create_dir(&data).unwrap();
    let mut cfg = small_cfg();
    cfg.shutdown_grace_ms = 80;
    cfg.kill_timeout_ms = 200;
    let sup = WorkerdSupervisor::new(
        WorkerdSupervisorOptions {
            runtime,
            compiler: compiler(data, "ignore_term", None, serde_json::json!({})),
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
    let snap = wait_state(&sup, SupervisorState::Running).await;
    let pid = snap.pid.unwrap();
    sup.shutdown().await;
    wait_reaped(pid, Duration::from_secs(3)).unwrap();
}
