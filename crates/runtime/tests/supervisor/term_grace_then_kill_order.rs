use super::*;

pub(super) async fn run() {
    let dir = TempDir::new().unwrap();
    let runtime = verified(dir.path()).await;
    let data = dir.path().join("d");
    fs::create_dir(&data).unwrap();
    clear_signal_log();
    let mut cfg = small_cfg();
    cfg.shutdown_grace_ms = 250;
    cfg.kill_timeout_ms = 400;
    cfg.drain_timeout_ms = 10;
    let sup = WorkerdSupervisor::new(
        WorkerdSupervisorOptions {
            runtime: runtime.clone(),
            compiler: compiler(data.clone(), "ready", None, serde_json::json!({})),
            config: cfg.clone(),
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
    let started = std::time::Instant::now();
    sup.shutdown().await;
    let elapsed = started.elapsed();
    wait_reaped(pid, Duration::from_secs(2)).unwrap();
    let signals = take_signal_log();
    let for_pid: Vec<_> = signals
        .iter()
        .filter(|(p, _)| *p == pid)
        .map(|(_, k)| *k)
        .collect();
    assert!(
        for_pid.contains(&"TERM"),
        "TERM-responsive child must receive TERM {signals:?}"
    );
    assert!(
        !for_pid.contains(&"KILL"),
        "TERM-responsive child must not receive KILL during grace {signals:?}"
    );
    assert!(
        elapsed < Duration::from_millis(250),
        "TERM-responsive shutdown should finish within grace, elapsed={elapsed:?}"
    );

    clear_signal_log();
    let data2 = dir.path().join("d2");
    fs::create_dir(&data2).unwrap();
    let mut cfg2 = cfg;
    cfg2.shutdown_grace_ms = 200;
    cfg2.kill_timeout_ms = 400;
    let sup = WorkerdSupervisor::new(
        WorkerdSupervisorOptions {
            runtime,
            compiler: compiler(data2, "ignore_term", None, serde_json::json!({})),
            config: cfg2,
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
    let started = std::time::Instant::now();
    sup.shutdown().await;
    let elapsed = started.elapsed();
    wait_reaped(pid, Duration::from_secs(3)).unwrap();
    let signals = take_signal_log();
    let for_pid: Vec<_> = signals
        .iter()
        .filter(|(p, _)| *p == pid)
        .map(|(_, k)| *k)
        .collect();
    let term_at = for_pid.iter().position(|k| *k == "TERM");
    let kill_at = for_pid.iter().position(|k| *k == "KILL");
    assert!(
        term_at.is_some(),
        "ignore_term must receive TERM {signals:?}"
    );
    assert!(
        kill_at.is_some(),
        "ignore_term must receive KILL {signals:?}"
    );
    assert!(
        kill_at.unwrap() > term_at.unwrap(),
        "KILL must follow TERM {signals:?}"
    );
    assert!(
        elapsed >= Duration::from_millis(180),
        "KILL must wait for grace, elapsed={elapsed:?}"
    );
    assert!(
        elapsed < Duration::from_millis(200 + 400 + 500),
        "KILL must complete within grace+kill deadline, elapsed={elapsed:?}"
    );
}
