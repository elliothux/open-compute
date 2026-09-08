use super::*;

pub(super) async fn run() {
    let dir = TempDir::new().unwrap();
    let runtime = verified(dir.path()).await;
    let data = dir.path().join("d");
    fs::create_dir(&data).unwrap();
    let pid;
    {
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
        pid = snap.pid.unwrap();
    }
    wait_reaped(pid, Duration::from_secs(3)).unwrap();
}
