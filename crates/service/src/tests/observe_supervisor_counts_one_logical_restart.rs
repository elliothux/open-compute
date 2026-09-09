use super::*;

#[test]
fn observe_supervisor_counts_one_logical_restart() {
    let reg = MetricsRegistry::new(&MetricsConfig::default(), "0.1.0", "unknown").unwrap();
    let mut snap = SupervisorSnapshot::initial_for_test(SystemTime::UNIX_EPOCH);
    assert_eq!(snap.state, SupervisorState::Stopped);
    assert_eq!(snap.attempt, 0);
    reg.observe_supervisor(&snap);
    snap.state = SupervisorState::Starting;
    snap.attempt = 1;
    reg.observe_supervisor(&snap);
    snap.state = SupervisorState::Running;
    snap.attempt = 1;
    reg.observe_supervisor(&snap);
    snap.state = SupervisorState::Running;
    reg.observe_supervisor(&snap);
    assert_eq!(reg.restart_total(RestartReason::UnexpectedExit), 0);
    assert_eq!(reg.restart_total(RestartReason::ProbeFailed), 0);

    snap.state = SupervisorState::BackingOff;
    snap.attempt = 1;
    reg.observe_supervisor(&snap);
    snap.state = SupervisorState::Starting;
    snap.attempt = 2;
    reg.observe_supervisor(&snap);
    snap.state = SupervisorState::Running;
    snap.attempt = 2;
    reg.observe_supervisor(&snap);
    assert_eq!(reg.restart_total(RestartReason::UnexpectedExit), 1);

    let reg = MetricsRegistry::new(&MetricsConfig::default(), "0.1.0", "unknown").unwrap();
    snap.state = SupervisorState::Running;
    snap.attempt = 1;
    reg.observe_supervisor(&snap);
    snap.state = SupervisorState::Running;
    snap.attempt = 2;
    reg.observe_supervisor(&snap);
    assert_eq!(
        reg.restart_total(RestartReason::UnexpectedExit),
        1,
        "coalesced Running -> BackingOff -> Running"
    );

    let reg = MetricsRegistry::new(&MetricsConfig::default(), "0.1.0", "unknown").unwrap();
    snap.state = SupervisorState::Starting;
    snap.attempt = 1;
    reg.observe_supervisor(&snap);
    snap.state = SupervisorState::Failed;
    snap.attempt = 1;
    reg.observe_supervisor(&snap);
    snap.state = SupervisorState::Failed;
    reg.observe_supervisor(&snap);
    assert_eq!(reg.restart_total(RestartReason::ProbeFailed), 1);
    assert_eq!(reg.restart_total(RestartReason::UnexpectedExit), 0);

    snap.state = SupervisorState::Draining;
    reg.observe_supervisor(&snap);
    snap.state = SupervisorState::Stopping;
    reg.observe_supervisor(&snap);
    assert_eq!(reg.restart_total(RestartReason::ProbeFailed), 1);
    assert_eq!(reg.restart_total(RestartReason::UnexpectedExit), 0);

    let reg = MetricsRegistry::new(&MetricsConfig::default(), "0.1.0", "unknown").unwrap();
    snap.state = SupervisorState::Starting;
    snap.attempt = 2;
    reg.observe_supervisor(&snap);
    snap.state = SupervisorState::Running;
    snap.attempt = 2;
    reg.observe_supervisor(&snap);
    assert_eq!(
        reg.restart_total(RestartReason::UnexpectedExit),
        1,
        "first observed snapshot already at attempt 2 is one restart"
    );
    snap.state = SupervisorState::Running;
    snap.attempt = 2;
    reg.observe_supervisor(&snap);
    assert_eq!(reg.restart_total(RestartReason::UnexpectedExit), 1);

    let reg = MetricsRegistry::new(&MetricsConfig::default(), "0.1.0", "unknown").unwrap();
    snap.state = SupervisorState::Starting;
    snap.attempt = 1;
    reg.observe_supervisor(&snap);
    snap.state = SupervisorState::Starting;
    snap.attempt = 3;
    reg.observe_supervisor(&snap);
    assert_eq!(
        reg.restart_total(RestartReason::UnexpectedExit),
        2,
        "coalesced attempt 1 -> 3 is two logical restarts"
    );

    let reg = MetricsRegistry::new(&MetricsConfig::default(), "0.1.0", "unknown").unwrap();
    snap.state = SupervisorState::Running;
    snap.attempt = 3;
    reg.observe_supervisor(&snap);
    assert_eq!(
        reg.restart_total(RestartReason::UnexpectedExit),
        2,
        "first observed snapshot at attempt 3 is two logical restarts"
    );
    snap.state = SupervisorState::Running;
    snap.attempt = 3;
    reg.observe_supervisor(&snap);
    assert_eq!(reg.restart_total(RestartReason::UnexpectedExit), 2);
}
