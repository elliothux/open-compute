use super::*;

pub(super) async fn run() {
    let dir = TempDir::new().unwrap();
    let runtime = verified(dir.path()).await;
    let data = dir.path().join("d");
    fs::create_dir(&data).unwrap();
    clear_signal_log();
    let _ = take_owner_wait_count();
    let pid;
    {
        let mut cfg = small_cfg();
        cfg.restart_budget = 1;
        let sup = WorkerdSupervisor::new(
            WorkerdSupervisorOptions {
                runtime,
                compiler: compiler(data, "crash_after_ready", None, serde_json::json!({})),
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
        pid = snap.pid.unwrap();
        wait_state(&sup, SupervisorState::Failed).await;
        wait_reaped(pid, Duration::from_secs(3)).unwrap();
        let waits_before_drop = take_owner_wait_count();
        assert!(waits_before_drop >= 1, "owner must reap once");
        let signals_before = take_signal_log();
        drop(sup);
        let signals_after = take_signal_log();
        assert!(
            signals_after.iter().all(|(p, _)| *p != pid),
            "Drop must not signal a reaped snapshot pid {signals_before:?} {signals_after:?}"
        );
        let waits_after = take_owner_wait_count();
        assert_eq!(waits_after, 0, "Drop must not double-wait");
    }
    assert_reaped(Some(pid)).unwrap();
}
