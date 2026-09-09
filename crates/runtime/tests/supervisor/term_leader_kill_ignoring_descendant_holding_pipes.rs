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
    cfg.kill_timeout_ms = 400;
    cfg.drain_timeout_ms = 10;
    clear_signal_log();
    let sup = WorkerdSupervisor::new(
        WorkerdSupervisorOptions {
            runtime,
            compiler: compiler(data, "child_ignore_term", None, extra),
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
    tokio::time::timeout(Duration::from_secs(5), sup.shutdown())
        .await
        .expect("shutdown must not hang on TERM-ignoring descendant holding pipes");
    wait_reaped(pid, Duration::from_secs(2)).unwrap();
    assert!(!pid_alive(descendant), "TERM-ignoring descendant leaked");
    assert!(!pid_alive(pid));
    let signals = take_signal_log();
    let kinds: Vec<_> = signals
        .iter()
        .filter(|(p, _)| *p == pid)
        .map(|(_, k)| *k)
        .collect();
    assert!(kinds.contains(&"TERM"), "expected TERM {signals:?}");
    assert!(
        kinds.contains(&"KILL"),
        "expected KILL after descendant survived TERM {signals:?}"
    );
    let term_at = kinds.iter().position(|k| *k == "TERM").unwrap();
    let kill_at = kinds.iter().position(|k| *k == "KILL").unwrap();
    assert!(kill_at > term_at, "KILL must follow TERM {signals:?}");
    assert_eq!(sup.owner_registry_len(), 0);
    let diag = sup.last_diagnostics();
    assert!(diag.is_some(), "readers must join and retain diagnostics");
}
