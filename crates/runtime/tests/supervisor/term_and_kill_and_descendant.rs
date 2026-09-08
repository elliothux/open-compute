use super::*;

pub(super) async fn run() {
    let dir = TempDir::new().unwrap();
    let runtime = verified(dir.path()).await;
    let data = dir.path().join("d");
    fs::create_dir(&data).unwrap();
    let child_pid_path = dir.path().join("child.pid");
    let extra = serde_json::json!({"child_pid_path": child_pid_path.display().to_string()});
    let mut cfg = small_cfg();
    cfg.shutdown_grace_ms = 150;
    let sup = WorkerdSupervisor::new(
        WorkerdSupervisorOptions {
            runtime,
            compiler: compiler(data, "child", None, extra),
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
    tokio::time::sleep(Duration::from_millis(50)).await;
    let descendant: i32 = fs::read_to_string(&child_pid_path)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    sup.shutdown().await;
    wait_reaped(pid, Duration::from_secs(2)).unwrap();
    assert!(!pid_alive(descendant), "descendant leaked");
}
