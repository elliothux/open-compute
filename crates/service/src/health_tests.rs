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

#[test]
fn search_background_failure_degrades_readiness_and_success_recovers() {
    let health = HealthCoordinator::new();
    for component in health.snapshot().components {
        health
            .set_component(
                component.name,
                ComponentState::Healthy,
                Some(ReadinessReason::Ready),
            )
            .unwrap();
    }
    health
        .set_search_background(ComponentName::VectorizeMutations, false)
        .unwrap();
    let failed = health.snapshot();
    let component = failed
        .components
        .iter()
        .find(|component| component.name == ComponentName::VectorizeMutations)
        .unwrap();
    assert_eq!(component.state, ComponentState::Degraded);
    assert_eq!(component.reason, Some(ReadinessReason::SearchUnavailable));
    assert_eq!(failed.readiness, ReadinessReason::SearchUnavailable);
    assert!(!failed.readiness.is_ready());

    health
        .set_search_background(ComponentName::VectorizeMutations, true)
        .unwrap();
    assert_eq!(
        health
            .snapshot()
            .components
            .into_iter()
            .find(|component| component.name == ComponentName::VectorizeMutations)
            .unwrap()
            .state,
        ComponentState::Healthy
    );
}
