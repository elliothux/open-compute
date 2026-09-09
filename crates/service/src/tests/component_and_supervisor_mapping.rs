use super::*;

#[test]
fn component_and_supervisor_mapping() {
    let mut status = PlatformStatus::starting();
    for c in &mut status.components {
        c.transition(ComponentState::Healthy, Some(ReadinessReason::Ready))
            .unwrap();
    }
    status.recompute();
    assert_eq!(status.readiness, ReadinessReason::Ready);

    let coord = HealthCoordinator::new();
    let now = SystemTime::UNIX_EPOCH;
    let mut snap = SupervisorSnapshot::initial_for_test(now);
    snap.state = SupervisorState::Starting;
    snap.reason = ReadinessReason::RuntimeStarting;
    coord.apply_supervisor(&snap).unwrap();
    assert_eq!(coord.readiness(), ReadinessReason::Starting);

    snap.state = SupervisorState::Running;
    snap.reason = ReadinessReason::Ready;
    coord.apply_supervisor(&snap).unwrap();
    assert_eq!(
        coord
            .snapshot()
            .components
            .iter()
            .find(|c| c.name == ComponentName::Runtime)
            .unwrap()
            .state,
        ComponentState::Healthy
    );

    snap.state = SupervisorState::BackingOff;
    snap.reason = ReadinessReason::RuntimeRestartBackoff;
    coord.apply_supervisor(&snap).unwrap();
    assert_eq!(coord.readiness(), ReadinessReason::RuntimeRestartBackoff);

    snap.state = SupervisorState::Failed;
    snap.reason = ReadinessReason::RuntimeInvalid;
    coord.apply_supervisor(&snap).unwrap();
    assert_eq!(coord.readiness(), ReadinessReason::RuntimeInvalid);

    snap.state = SupervisorState::Starting;
    snap.reason = ReadinessReason::RuntimeStarting;
    coord.apply_supervisor(&snap).unwrap();

    snap.state = SupervisorState::Draining;
    coord.apply_supervisor(&snap).unwrap();
    assert_eq!(coord.readiness(), ReadinessReason::Draining);

    let stopped = SupervisorSnapshot::initial_for_test(now);
    assert_eq!(
        map_supervisor(&stopped),
        (ComponentState::Starting, ReadinessReason::Starting)
    );
    snap.state = SupervisorState::Failed;
    snap.reason = ReadinessReason::ObjectStorageUnavailable;
    assert_eq!(
        map_supervisor(&snap),
        (
            ComponentState::Failed,
            ReadinessReason::ObjectStorageUnavailable
        )
    );

    let degraded = HealthCoordinator::default();
    degraded
        .set_component(
            ComponentName::Runtime,
            ComponentState::Degraded,
            Some(ReadinessReason::RuntimeRestartBackoff),
        )
        .unwrap();
    snap.state = SupervisorState::Starting;
    snap.reason = ReadinessReason::RuntimeStarting;
    degraded.apply_supervisor(&snap).unwrap();
    assert_eq!(runtime_state(&degraded), ComponentState::Starting);
    degraded.begin_drain().unwrap();
    degraded.begin_drain().unwrap();
}
