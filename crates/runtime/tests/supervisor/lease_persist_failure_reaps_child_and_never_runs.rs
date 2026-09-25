use super::*;

pub(super) async fn run() {
    let dir = TempDir::new().unwrap();
    let runtime = verified(dir.path()).await;
    let data = dir.path().join("d");
    fs::create_dir(&data).unwrap();
    fs::create_dir(data.join("runtime")).unwrap();
    let lease = data.join("runtime/child.lease");
    open_compute_runtime::set_start_key_hook(Some(test_start_key));
    open_compute_runtime::set_lease_write_fail(true);
    let sup = WorkerdSupervisor::new(
        WorkerdSupervisorOptions {
            runtime,
            compiler: compiler(data, "ready", None, serde_json::json!({})),
            config: small_cfg(),
            clock: Arc::new(DeterministicClock::new(UNIX_EPOCH)),
            jitter: Arc::new(SequenceJitter::new(vec![0])),
            redactor: Redactor::new(),
            lease_path: Some(lease),
        },
        Vec::new(),
        Vec::new(),
        Vec::new(),
    );
    sup.start();
    let snap = wait_state(&sup, SupervisorState::Failed).await;
    assert_ne!(snap.state, SupervisorState::Running);
    let pid = last_spawned_pid().expect("lease persistence failure must follow spawn");
    wait_reaped(pid, Duration::from_secs(3)).unwrap();
    open_compute_runtime::set_lease_write_fail(false);
    open_compute_runtime::set_start_key_hook(None);
    sup.shutdown().await;
}
