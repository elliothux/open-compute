use super::*;

pub(super) async fn run() {
    let dir = TempDir::new_in("/tmp").unwrap();
    let runtime = verified(dir.path()).await;
    let data = dir.path().join("d");
    fs::create_dir(&data).unwrap();
    let sup = WorkerdSupervisor::new(
        WorkerdSupervisorOptions {
            runtime,
            compiler: compiler(data, "ready", None, serde_json::json!({})),
            config: small_cfg(),
            clock: Arc::new(DeterministicClock::new(UNIX_EPOCH)),
            jitter: Arc::new(SequenceJitter::new(vec![0])),
            lease_path: None,
        },
        Vec::new(),
        Vec::new(),
    );
    sup.start();
    let before = wait_state(&sup, SupervisorState::Running).await;
    let old_pid = before.pid.unwrap();
    sup.rotate_generation(Duration::from_secs(5)).await.unwrap();
    let after = sup.snapshot();
    assert_eq!(after.state, SupervisorState::Running);
    assert_ne!(after.startup_id, before.startup_id);
    assert_ne!(after.pid, before.pid);
    wait_reaped(old_pid, Duration::from_secs(2)).unwrap();
    sup.shutdown().await;
}
