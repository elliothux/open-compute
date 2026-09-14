use super::*;

/// Suspicion against a healthy generation must be answered by one functional probe that
/// succeeds: no restart, no budget consumption, and the child identity is unchanged.
pub(crate) mod suspicion_healthy_probe_does_not_restart {
    use super::*;
    pub(crate) async fn run() {
        let dir = TempDir::new().unwrap();
        let runtime = verified(dir.path()).await;
        let data = dir.path().join("d");
        fs::create_dir(&data).unwrap();
        let cfg = small_cfg();
        let clock = Arc::new(DeterministicClock::new(UNIX_EPOCH));
        let sup = WorkerdSupervisor::new(
            WorkerdSupervisorOptions {
                runtime,
                compiler: compiler(data, "ready", None, serde_json::json!({})),
                config: cfg,
                clock,
                jitter: Arc::new(SequenceJitter::new(vec![0])),
                redactor: Redactor::new(),
                lease_path: None,
            },
            Vec::new(),
            Vec::new(),
            Vec::new(),
        );
        sup.set_watchdog_for_test(WatchdogConfig {
            probe_interval: Duration::from_millis(50),
            probe_timeout: Duration::from_millis(500),
            failure_threshold: 3,
        });
        sup.start();
        let running =
            wait_state_within(&sup, SupervisorState::Running, Duration::from_secs(10)).await;
        let startup_id = running.startup_id.expect("running startup id");
        let pid = running.pid.expect("running pid");
        let attempt = running.attempt;

        // Evidence storm against the same healthy generation: all of it merges into probes
        // that keep succeeding.
        for _ in 0..32 {
            sup.suspect_unhealthy(startup_id, RuntimeFailureEvidence::ResponseHeaderTimeout);
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
        let snap = sup.snapshot();
        assert_eq!(snap.state, SupervisorState::Running, "{snap:?}");
        assert_eq!(snap.pid, Some(pid));
        assert_eq!(snap.startup_id, Some(startup_id));
        assert_eq!(snap.attempt, attempt, "no budget was consumed");
        sup.shutdown().await;
        wait_reaped(pid, Duration::from_secs(5)).unwrap();
    }
}

/// A wedged event loop (port and control channel alive, `/internal/live` unresponsive) is
/// detected by the periodic probe and recovered with exactly one teardown and one budget
/// consumption per confirmed fault.
pub(crate) mod stalled_event_loop_restarts_once {
    use super::*;
    pub(crate) async fn run() {
        let dir = TempDir::new().unwrap();
        let runtime = verified(dir.path()).await;
        let data = dir.path().join("d");
        fs::create_dir(&data).unwrap();
        let cfg = small_cfg();
        let clock = Arc::new(DeterministicClock::new(UNIX_EPOCH));
        let sup = WorkerdSupervisor::new(
            WorkerdSupervisorOptions {
                runtime,
                compiler: compiler(data, "stall_live", None, serde_json::json!({})),
                config: cfg,
                clock: clock.clone(),
                jitter: Arc::new(SequenceJitter::new(vec![0, 0])),
                redactor: Redactor::new(),
                lease_path: None,
            },
            Vec::new(),
            Vec::new(),
            Vec::new(),
        );
        sup.set_watchdog_for_test(WatchdogConfig {
            probe_interval: Duration::from_millis(50),
            probe_timeout: Duration::from_millis(200),
            failure_threshold: 3,
        });
        sup.start();
        let running =
            wait_state_within(&sup, SupervisorState::Running, Duration::from_secs(10)).await;
        let stalled_id = running.startup_id.expect("running startup id");
        let stalled_pid = running.pid.expect("running pid");
        let attempt_before = running.attempt;

        // The wedged generation never answers; the periodic probe must confirm and restart.
        // The deterministic clock only advances when driven, so tick it past each probe and
        // backoff deadline while waiting.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
        let recovered = loop {
            clock.advance(Duration::from_millis(20));
            let snap = sup.snapshot();
            if snap.state == SupervisorState::Running && snap.startup_id != Some(stalled_id) {
                break snap;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "stalled generation was not recovered, last={snap:?}"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        };
        // One confirmed fault consumes exactly one restart: the attempt counter advanced by one.
        assert_eq!(recovered.attempt, attempt_before + 1, "{recovered:?}");
        // The stalled child was torn down and reaped.
        wait_reaped(stalled_pid, Duration::from_secs(5)).unwrap();
        assert_reaped(Some(stalled_pid)).unwrap();
        // The old generation's late evidence cannot touch the new child.
        for _ in 0..8 {
            sup.suspect_unhealthy(stalled_id, RuntimeFailureEvidence::PeriodicProbeFailed);
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
        let snap = sup.snapshot();
        assert_eq!(snap.state, SupervisorState::Running, "{snap:?}");
        assert_eq!(snap.startup_id, recovered.startup_id);
        assert_eq!(
            snap.attempt, recovered.attempt,
            "stale evidence consumed no budget"
        );
        sup.shutdown().await;
    }
}

/// Suspicion for an unknown generation is dropped without probing or restarting.
pub(crate) mod stale_suspicion_is_dropped {
    use super::*;
    pub(crate) async fn run() {
        let dir = TempDir::new().unwrap();
        let runtime = verified(dir.path()).await;
        let data = dir.path().join("d");
        fs::create_dir(&data).unwrap();
        let cfg = small_cfg();
        let clock = Arc::new(DeterministicClock::new(UNIX_EPOCH));
        let sup = WorkerdSupervisor::new(
            WorkerdSupervisorOptions {
                runtime,
                compiler: compiler(data, "ready", None, serde_json::json!({})),
                config: cfg,
                clock,
                jitter: Arc::new(SequenceJitter::new(vec![0])),
                redactor: Redactor::new(),
                lease_path: None,
            },
            Vec::new(),
            Vec::new(),
            Vec::new(),
        );
        sup.start();
        let running =
            wait_state_within(&sup, SupervisorState::Running, Duration::from_secs(10)).await;
        let pid = running.pid.expect("running pid");

        let unknown = StartupId::generate();
        sup.suspect_unhealthy(unknown, RuntimeFailureEvidence::ConnectFailed);
        tokio::time::sleep(Duration::from_millis(200)).await;
        let snap = sup.snapshot();
        assert_eq!(snap.state, SupervisorState::Running, "{snap:?}");
        assert_eq!(snap.pid, Some(pid));
        assert_eq!(
            snap.attempt, running.attempt,
            "stale suspicion consumed no budget"
        );
        sup.shutdown().await;
        wait_reaped(pid, Duration::from_secs(5)).unwrap();
    }
}

pub(crate) mod drain_lifecycle {
    use super::*;

    pub(crate) async fn run() {
        let dir = TempDir::new().unwrap();
        let runtime = verified(dir.path()).await;
        let data = dir.path().join("d");
        fs::create_dir(&data).unwrap();
        let cfg = small_cfg();
        let clock = Arc::new(DeterministicClock::new(UNIX_EPOCH));
        let sup = WorkerdSupervisor::new(
            WorkerdSupervisorOptions {
                runtime,
                compiler: compiler(data, "ready", None, serde_json::json!({})),
                config: cfg,
                clock,
                jitter: Arc::new(SequenceJitter::new(vec![0])),
                redactor: Redactor::new(),
                lease_path: None,
            },
            Vec::new(),
            Vec::new(),
            Vec::new(),
        );
        sup.start();
        let running =
            wait_state_within(&sup, SupervisorState::Running, Duration::from_secs(10)).await;
        let pid = running.pid.expect("running pid");

        // Draining a Running generation stops gracefully and lands in Stopped without
        // consuming restart budget or failing.
        sup.begin_drain();
        let stopped =
            wait_state_within(&sup, SupervisorState::Stopped, Duration::from_secs(10)).await;
        assert_eq!(stopped.reason, ReadinessReason::Draining);
        assert!(stopped.pid.is_none());
        wait_reaped(pid, Duration::from_secs(5)).unwrap();
        sup.shutdown().await;
    }

    pub(crate) async fn run_during_startup() {
        let dir = TempDir::new().unwrap();
        let runtime = verified(dir.path()).await;
        let data = dir.path().join("d");
        fs::create_dir(&data).unwrap();
        // The "no_control" fixture never reaches readiness, keeping the attempt in
        // flight deterministically until the drain cancels it.
        let cfg = small_cfg();
        let sup = WorkerdSupervisor::new(
            WorkerdSupervisorOptions {
                runtime,
                compiler: compiler(data, "no_control", None, serde_json::json!({})),
                config: cfg,
                clock: Arc::new(open_compute_core::SystemClock),
                jitter: Arc::new(SequenceJitter::new(vec![0])),
                redactor: Redactor::new(),
                lease_path: None,
            },
            Vec::new(),
            Vec::new(),
            Vec::new(),
        );
        sup.start();
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(sup.snapshot().state, SupervisorState::Starting);
        // Draining during startup cancels the attempt and stops without a restart.
        sup.begin_drain();
        let stopped =
            wait_state_within(&sup, SupervisorState::Stopped, Duration::from_secs(10)).await;
        assert_eq!(stopped.reason, ReadinessReason::Draining);
        assert!(stopped.pid.is_none());
        sup.shutdown().await;
    }
}
