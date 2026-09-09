use super::*;

pub(super) async fn run() {
    let dir = TempDir::new().unwrap();
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
    wait_state(&sup, SupervisorState::Stopped).await;
    assert!(snap.last_exit.is_none() || !snap.last_exit.as_ref().unwrap().retryable);
    wait_reaped(pid, Duration::from_secs(2)).unwrap();
    sup.shutdown().await;
    let after = sup.snapshot();
    assert_eq!(after.state, SupervisorState::Stopped);
    assert!(
        after
            .last_exit
            .as_ref()
            .is_none_or(|e| e.code_name != "RUNTIME_INVALID"),
        "clean shutdown must not be classified as runtime invalid: {:?}",
        after.last_exit
    );
}
