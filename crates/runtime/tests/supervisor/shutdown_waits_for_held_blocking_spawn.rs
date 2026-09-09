use super::*;

pub(super) async fn run() {
    let dir = TempDir::new().unwrap();
    let runtime = verified(dir.path()).await;
    let data = dir.path().join("d");
    fs::create_dir(&data).unwrap();
    clear_blocking_spawn_hold();
    hold_blocking_spawn();
    let sup = Arc::new(WorkerdSupervisor::new(
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
    ));
    sup.start();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    while !blocking_spawn_is_waiting() {
        if tokio::time::Instant::now() > deadline {
            clear_blocking_spawn_hold();
            panic!("spawn never entered the blocking hold");
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let shut_sup = sup.clone();
    let shut = tokio::spawn(async move { shut_sup.shutdown().await });
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        !shut.is_finished(),
        "shutdown must not acknowledge while blocking spawn is held"
    );
    release_blocking_spawn();
    tokio::time::timeout(Duration::from_secs(5), async { shut.await.unwrap() })
        .await
        .expect("shutdown after release");
    clear_blocking_spawn_hold();
    assert_eq!(sup.snapshot().state, SupervisorState::Stopped);
    assert_eq!(sup.owner_registry_len(), 0);
    if let Some(pid) = last_spawned_pid() {
        wait_reaped(pid, Duration::from_secs(3)).unwrap();
        assert!(!pid_alive(pid));
    }
}
