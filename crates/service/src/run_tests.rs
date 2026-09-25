use super::*;

#[test]
fn scoped_daemon_requires_existing_nonroot_uid_owned_root() {
    let temp = tempfile::tempdir().unwrap();
    assert_eq!(
        validate_scope_runtime_owner(&temp.path().join("missing"))
            .unwrap_err()
            .code(),
        ErrorCode::PathInvalid,
    );
    if rustix::process::getuid().is_root() {
        assert_eq!(
            validate_scope_runtime_owner(temp.path())
                .unwrap_err()
                .code(),
            ErrorCode::PathInvalid,
        );
    } else {
        validate_scope_runtime_owner(temp.path()).unwrap();
    }
}

#[test]
fn do_storage_health_tracks_watermarks_and_component_state() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("keys")).unwrap();
    let data = open_compute_core::config::DataConfig {
        path: root.path().join("data"),
        master_key_file: root.path().join("keys/master.key"),
        master_key_env: None,
        sqlite_busy_timeout_ms: 5_000,
        free_space_soft_bytes: 1,
        free_space_hard_bytes: 1,
    };
    let storage = PlatformStorage::bootstrap(&data, &SystemClock).unwrap();
    let health = HealthCoordinator::new();
    let metrics = MetricsRegistry::new(
        &open_compute_core::MetricsConfig::default(),
        "test",
        "workerd",
    )
    .unwrap();
    let component = || {
        health
            .snapshot()
            .components
            .into_iter()
            .find(|component| component.name == ComponentName::DataDir)
            .unwrap()
    };

    // Real disk usage on the test host is nonzero; drive each watermark explicitly so
    // the assertions do not depend on the host's actual filesystem usage. The clear
    // policy must place both watermarks above the clampable used-percent domain;
    // inheriting the stop-writes default would re-introduce host dependence.
    let clear = open_compute_core::DurableObjectsConfig {
        disk_high_watermark_percent: 101,
        disk_stop_writes_percent: 102,
        ..open_compute_core::DurableObjectsConfig::default()
    };
    update_do_storage_health(&storage, &clear, &health, &metrics).unwrap();
    assert_eq!(component().state, ComponentState::Healthy);
    assert_eq!(component().reason, Some(ReadinessReason::Ready));

    // A soft watermark below real usage reports the soft limit while the stop-writes
    // threshold stays above it.
    let soft = open_compute_core::DurableObjectsConfig {
        disk_high_watermark_percent: 1,
        disk_stop_writes_percent: 101,
        ..open_compute_core::DurableObjectsConfig::default()
    };
    update_do_storage_health(&storage, &soft, &health, &metrics).unwrap();
    assert_eq!(component().state, ComponentState::Degraded);
    assert_eq!(component().reason, Some(ReadinessReason::DiskSoftLimit));

    // A policy at the stop-writes watermark reports the hard limit.
    let hard = open_compute_core::DurableObjectsConfig {
        disk_high_watermark_percent: 1,
        disk_stop_writes_percent: 2,
        ..open_compute_core::DurableObjectsConfig::default()
    };
    update_do_storage_health(&storage, &hard, &health, &metrics).unwrap();
    assert_eq!(component().state, ComponentState::Degraded);
    assert_eq!(component().reason, Some(ReadinessReason::DiskHardLimit));
}
