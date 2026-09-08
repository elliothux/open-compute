use super::*;

pub(super) async fn run() {
    let dir = TempDir::new().unwrap();
    let runtime = verified(dir.path()).await;
    let data = dir.path().join("teardown-proof");
    fs::create_dir(&data).unwrap();
    let lease = dir.path().join("child.lease");
    open_compute_runtime::set_start_key_hook(Some(test_start_key));

    let sup = WorkerdSupervisor::new(
        WorkerdSupervisorOptions {
            runtime: runtime.clone(),
            compiler: compiler(data.clone(), "ready", None, serde_json::json!({})),
            config: small_cfg(),
            clock: Arc::new(DeterministicClock::new(UNIX_EPOCH)),
            jitter: Arc::new(SequenceJitter::new(vec![0])),
            redactor: Redactor::new(),
            lease_path: Some(lease.clone()),
        },
        Vec::new(),
        Vec::new(),
        Vec::new(),
    );
    sup.start();
    let running = wait_state(&sup, SupervisorState::Running).await;
    let pid = running.pid.unwrap();
    assert!(lease.exists());

    open_compute_runtime::set_reap_probe_fail(true);
    sup.report_unhealthy();
    let failed = wait_state(&sup, SupervisorState::Failed).await;
    assert_eq!(failed.reason, ReadinessReason::RuntimeInvalid);
    assert!(lease.exists(), "failed reap proof must retain the lease");
    let attempts = failed.attempt;
    sup.start();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        sup.snapshot().attempt,
        attempts,
        "fail-closed state must not restart"
    );

    open_compute_runtime::set_reap_probe_fail(false);
    wait_reaped(pid, Duration::from_secs(3)).unwrap();
    sup.shutdown().await;
    assert!(
        lease.exists(),
        "the same actor must not clear a lease after losing its reap proof"
    );

    let replacement = WorkerdSupervisor::new(
        WorkerdSupervisorOptions {
            runtime,
            compiler: compiler(data, "ready", None, serde_json::json!({})),
            config: small_cfg(),
            clock: Arc::new(DeterministicClock::new(UNIX_EPOCH)),
            jitter: Arc::new(SequenceJitter::new(vec![0])),
            redactor: Redactor::new(),
            lease_path: Some(lease.clone()),
        },
        Vec::new(),
        Vec::new(),
        Vec::new(),
    );
    replacement.start();
    let next = wait_state(&replacement, SupervisorState::Running).await;
    assert_ne!(next.pid, Some(pid));
    replacement.shutdown().await;
    assert!(!lease.exists());
    open_compute_runtime::set_start_key_hook(None);
}
