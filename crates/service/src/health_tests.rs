use super::*;

fn runtime_index(status: &PlatformStatus) -> usize {
    status
        .components
        .iter()
        .position(|component| component.name == ComponentName::Runtime)
        .unwrap()
}

#[test]
fn bridged_health_paths_preserve_legal_state_machine_edges() {
    for (previous, desired) in [
        (ComponentState::Failed, ComponentState::Healthy),
        (ComponentState::Healthy, ComponentState::Starting),
        (ComponentState::Degraded, ComponentState::Starting),
    ] {
        let mut status = PlatformStatus::starting();
        let idx = runtime_index(&status);
        status.components[idx].state = previous;
        status.components[idx].reason = Some(ReadinessReason::RuntimeInvalid);
        apply_bridged(
            &mut status,
            idx,
            previous,
            desired,
            ReadinessReason::RuntimeStarting,
        )
        .unwrap();
        assert_eq!(status.components[idx].state, desired);
    }

    let mut same = PlatformStatus::starting();
    let idx = runtime_index(&same);
    apply_bridged(
        &mut same,
        idx,
        ComponentState::Starting,
        ComponentState::Starting,
        ReadinessReason::RuntimeStarting,
    )
    .unwrap();
    assert_eq!(
        same.components[idx].reason,
        Some(ReadinessReason::RuntimeStarting)
    );

    let mut illegal = PlatformStatus::starting();
    let idx = runtime_index(&illegal);
    illegal.components[idx].state = ComponentState::Draining;
    assert_eq!(
        apply_bridged(
            &mut illegal,
            idx,
            ComponentState::Draining,
            ComponentState::Healthy,
            ReadinessReason::Ready,
        )
        .unwrap_err()
        .code(),
        ErrorCode::ConfigInvalid
    );
    assert_eq!(illegal.components[idx].state, ComponentState::Failed);
}
