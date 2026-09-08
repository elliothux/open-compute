use super::*;

#[test]
fn coalesced_watch_transitions_and_draining_terminal() {
    let now = SystemTime::UNIX_EPOCH;
    let coord = HealthCoordinator::new();
    let mut snap = SupervisorSnapshot::initial_for_test(now);
    snap.state = SupervisorState::Failed;
    snap.reason = ReadinessReason::RuntimeInvalid;
    coord.apply_supervisor(&snap).unwrap();
    snap.state = SupervisorState::Running;
    snap.reason = ReadinessReason::Ready;
    coord.apply_supervisor(&snap).unwrap();
    assert_eq!(
        runtime_state(&coord),
        ComponentState::Healthy,
        "Failed -> Running must bridge through Starting"
    );
    assert_eq!(coord.readiness(), ReadinessReason::Starting); // other components still starting
    snap.state = SupervisorState::Running;
    coord.apply_supervisor(&snap).unwrap();
    assert_eq!(runtime_state(&coord), ComponentState::Healthy);

    let coord = HealthCoordinator::new();
    snap.state = SupervisorState::Running;
    coord.apply_supervisor(&snap).unwrap();
    snap.state = SupervisorState::Starting;
    snap.reason = ReadinessReason::RuntimeStarting;
    coord.apply_supervisor(&snap).unwrap();
    assert_eq!(runtime_state(&coord), ComponentState::Starting);
    snap.state = SupervisorState::Running;
    snap.reason = ReadinessReason::Ready;
    coord.apply_supervisor(&snap).unwrap();
    assert_eq!(runtime_state(&coord), ComponentState::Healthy);

    let coord = HealthCoordinator::new();
    snap.state = SupervisorState::BackingOff;
    snap.reason = ReadinessReason::RuntimeRestartBackoff;
    coord.apply_supervisor(&snap).unwrap();
    snap.state = SupervisorState::Running;
    snap.reason = ReadinessReason::Ready;
    coord.apply_supervisor(&snap).unwrap();
    assert_eq!(runtime_state(&coord), ComponentState::Healthy);

    let coord = HealthCoordinator::new();
    snap.state = SupervisorState::Draining;
    coord.apply_supervisor(&snap).unwrap();
    snap.state = SupervisorState::Running;
    let _ = coord.apply_supervisor(&snap);
    assert_eq!(runtime_state(&coord), ComponentState::Draining);
    assert_eq!(coord.readiness(), ReadinessReason::Draining);

    let (st, reason) = map_supervisor(&SupervisorSnapshot {
        state: SupervisorState::Stopping,
        reason: ReadinessReason::Draining,
        last_transition_at: now,
        attempt: 1,
        last_exit: None,
        next_retry_at: None,
        pid: Some(1),
        pgid: Some(1),
        binary_digest: "x".into(),
        config_digest: "y".into(),
        startup_id: None,
        token_fingerprint: None,
        listen_port: Some(1),
    });
    assert_eq!(st, ComponentState::Draining);
    assert_eq!(reason, ReadinessReason::Draining);
}
