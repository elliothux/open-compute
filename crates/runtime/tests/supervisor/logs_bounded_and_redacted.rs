use super::*;

pub(super) async fn run() {
    let dir = TempDir::new().unwrap();
    let runtime = verified(dir.path()).await;
    let data = dir.path().join("d");
    fs::create_dir(&data).unwrap();
    let mut redactor = Redactor::new();
    redactor.register_str("/secret/token-path");
    let mut cfg = small_cfg();
    cfg.restart_budget = 1;
    let sup = WorkerdSupervisor::new(
        WorkerdSupervisorOptions {
            runtime,
            compiler: compiler(data, "secret_logs", None, serde_json::json!({})),
            config: cfg,
            clock: Arc::new(DeterministicClock::new(UNIX_EPOCH)),
            jitter: Arc::new(SequenceJitter::new(vec![0])),
            redactor,
            lease_path: None,
        },
        Vec::new(),
        Vec::new(),
        Vec::new(),
    );
    sup.start();
    let snap = wait_state(&sup, SupervisorState::Running).await;
    let token_in_config = snap.token_fingerprint.clone();
    let _ = token_in_config;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let now = sup.snapshot();
        if now.state == SupervisorState::Failed || now.state == SupervisorState::BackingOff {
            break;
        }
        if tokio::time::Instant::now() > deadline {
            panic!("secret_logs did not exit, last={now:?}");
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let snap = sup.snapshot();
    let debug = format!("{snap:?}");
    let status = serde_json::to_string(&snap).unwrap();
    assert!(!debug.contains("Authorization"));
    assert!(!debug.to_lowercase().contains("token="));
    assert!(!debug.contains("/secret/token-path"));
    assert!(!status.contains("token_fingerprint"));
    let exit = snap.last_exit.expect("sanitized exit");
    assert_eq!(exit.code, Some(42));
    assert_eq!(exit.signal, None);
    let diag = sup.last_diagnostics().expect("diagnostics");
    assert!(diag.stdout_tail.len() <= 16 * 1024);
    assert!(diag.stderr_tail.len() <= 16 * 1024);
    assert!(!diag.stdout_tail.contains("Authorization"));
    assert!(!diag.stderr_tail.contains("Authorization: Bearer"));
    assert!(!diag.stdout_tail.contains("/secret/token-path"));
    assert!(!diag.stderr_tail.contains("/secret/token-path"));
    assert!(
        diag.stderr_tail.contains("[REDACTED]") || diag.stdout_tail.contains("[REDACTED]"),
        "headers must be redacted"
    );
    assert!(
        !diag.stderr_tail.contains(&"A".repeat(9000)),
        "oversized lines must be bounded"
    );
    assert!(
        diag.stderr_tail.contains('\u{fffd}') || diag.stdout_tail.contains('\u{fffd}'),
        "invalid utf8 must be lossy-redacted"
    );
    sup.shutdown().await;
}
