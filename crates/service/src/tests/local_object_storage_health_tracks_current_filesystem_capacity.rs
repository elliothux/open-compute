use super::*;

#[test]
fn local_object_storage_health_tracks_current_filesystem_capacity() {
    let temp = TempDir::new().unwrap();
    let mut local = open_compute_core::LocalObjectStorageConfig {
        path: temp.path().join("objects"),
        free_space_soft_bytes: 1,
        free_space_hard_bytes: 1,
        ..open_compute_core::LocalObjectStorageConfig::default()
    };
    let backend = open_compute_artifacts::ObjectBackend::open_local(
        &local,
        open_compute_core::PlatformId::generate(),
        1024,
    )
    .unwrap();
    let health = HealthCoordinator::new();
    local.free_space_hard_bytes = u64::MAX;
    local.free_space_soft_bytes = u64::MAX;
    update_local_object_storage_health(
        &backend,
        &open_compute_core::ObjectStorageConfig::Local(local.clone()),
        &health,
    )
    .unwrap();
    let component = || {
        health
            .snapshot()
            .components
            .into_iter()
            .find(|component| component.name == ComponentName::ObjectStorage)
            .unwrap()
    };
    assert_eq!(component().reason, Some(ReadinessReason::DiskHardLimit));

    local.free_space_hard_bytes = 1;
    update_local_object_storage_health(
        &backend,
        &open_compute_core::ObjectStorageConfig::Local(local.clone()),
        &health,
    )
    .unwrap();
    assert_eq!(component().reason, Some(ReadinessReason::DiskSoftLimit));

    local.free_space_soft_bytes = 1;
    update_local_object_storage_health(
        &backend,
        &open_compute_core::ObjectStorageConfig::Local(local),
        &health,
    )
    .unwrap();
    assert_eq!(component().state, ComponentState::Healthy);
    assert_eq!(component().reason, Some(ReadinessReason::Ready));
}
