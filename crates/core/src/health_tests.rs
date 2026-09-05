use super::*;

#[test]
fn health_transitions_and_recompute() {
    let mut status = PlatformStatus::starting();
    assert_eq!(status.readiness, ReadinessReason::Starting);
    for component in &mut status.components {
        component
            .transition(ComponentState::Healthy, Some(ReadinessReason::Ready))
            .expect("start->healthy");
    }
    status.recompute();
    assert_eq!(status.readiness, ReadinessReason::Ready);

    status.components[4]
        .transition(
            ComponentState::Failed,
            Some(ReadinessReason::ObjectStorageUnavailable),
        )
        .expect("healthy->failed");
    status.recompute();
    assert_eq!(status.readiness, ReadinessReason::ObjectStorageUnavailable);

    status.components[0]
        .transition(ComponentState::Draining, Some(ReadinessReason::Draining))
        .expect("healthy->draining");
    status.recompute();
    assert_eq!(status.readiness, ReadinessReason::Draining);
}

#[test]
fn degraded_required_object_storage_is_unready_but_bounded_disk_pressure_is_serviceable() {
    let mut status = PlatformStatus::starting();
    for component in &mut status.components {
        component
            .transition(ComponentState::Healthy, Some(ReadinessReason::Ready))
            .expect("start->healthy");
    }
    status.recompute();
    assert_eq!(status.readiness, ReadinessReason::Ready);

    let s3 = status
        .components
        .iter_mut()
        .find(|c| c.name == ComponentName::ObjectStorage)
        .expect("s3");
    s3.transition(
        ComponentState::Degraded,
        Some(ReadinessReason::ObjectStorageUnavailable),
    )
    .expect("healthy->degraded");
    status.recompute();
    assert_eq!(status.readiness, ReadinessReason::ObjectStorageUnavailable);
    assert!(!status.readiness.is_ready());

    let s3 = status
        .components
        .iter_mut()
        .find(|c| c.name == ComponentName::ObjectStorage)
        .expect("s3");
    s3.transition(ComponentState::Healthy, Some(ReadinessReason::Ready))
        .expect("degraded->healthy");
    status.recompute();
    assert_eq!(status.readiness, ReadinessReason::Ready);

    let cache = status
        .components
        .iter_mut()
        .find(|c| c.name == ComponentName::Cache)
        .expect("cache");
    cache
        .transition(
            ComponentState::Degraded,
            Some(ReadinessReason::DiskHardLimit),
        )
        .expect("healthy->degraded");
    status.recompute();
    assert_eq!(status.readiness, ReadinessReason::DiskHardLimit);
    assert!(status.readiness.is_ready());
}

#[test]
fn illegal_transition_is_rejected() {
    let mut health = ComponentHealth::starting(ComponentName::Runtime);
    health
        .transition(ComponentState::Healthy, Some(ReadinessReason::Ready))
        .unwrap();
    let err = health
        .transition(ComponentState::Starting, Some(ReadinessReason::Starting))
        .unwrap_err();
    assert_eq!(err.code(), crate::error::ErrorCode::ConfigInvalid);
}

#[test]
fn component_labels_and_transition_matrix_are_complete() {
    let names = [
        ComponentName::Process,
        ComponentName::DataDir,
        ComponentName::ControlDb,
        ComponentName::MasterKey,
        ComponentName::ObjectStorage,
        ComponentName::Cache,
        ComponentName::Runtime,
        ComponentName::Scheduler,
        ComponentName::Operations,
        ComponentName::VectorizeStorage,
        ComponentName::VectorizeMutations,
        ComponentName::AiSearchStorage,
        ComponentName::AiSearchIndexing,
        ComponentName::AiModels,
    ];
    for name in names {
        assert_eq!(name.to_string(), name.as_str());
    }
    let states = [
        ComponentState::Starting,
        ComponentState::Healthy,
        ComponentState::Degraded,
        ComponentState::Failed,
        ComponentState::Draining,
    ];
    for state in states {
        assert!(!state.as_str().is_empty());
        assert!(state.can_transition_to(state));
    }
    assert!(ComponentState::Failed.can_transition_to(ComponentState::Starting));
    assert!(ComponentState::Draining.can_transition_to(ComponentState::Failed));
    assert!(!ComponentState::Draining.can_transition_to(ComponentState::Healthy));
}
