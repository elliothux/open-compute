use super::*;

#[test]
fn scoped_daemon_requires_nonroot_user() {
    if rustix::process::getuid().is_root() {
        assert_eq!(
            validate_runtime_user().unwrap_err().code(),
            ErrorCode::PathInvalid
        );
    } else {
        validate_runtime_user().unwrap();
    }
}

#[test]
fn do_storage_health_tracks_watermarks_and_component_state() {
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

    let config = open_compute_core::DurableObjectsConfig::default();
    let stop = config.disk_stop_writes_free_bytes;
    let degraded = config.disk_degraded_free_bytes;
    for (available, state, reason, active_label) in [
        (
            stop - 1,
            ComponentState::Degraded,
            ReadinessReason::DiskHardLimit,
            "stop",
        ),
        (
            stop,
            ComponentState::Degraded,
            ReadinessReason::DiskSoftLimit,
            "high",
        ),
        (
            stop + 1,
            ComponentState::Degraded,
            ReadinessReason::DiskSoftLimit,
            "high",
        ),
        (
            degraded - 1,
            ComponentState::Degraded,
            ReadinessReason::DiskSoftLimit,
            "high",
        ),
        (
            degraded,
            ComponentState::Healthy,
            ReadinessReason::Ready,
            "normal",
        ),
        (
            degraded + 1,
            ComponentState::Healthy,
            ReadinessReason::Ready,
            "normal",
        ),
        (
            stop - 1,
            ComponentState::Degraded,
            ReadinessReason::DiskHardLimit,
            "stop",
        ),
        (
            degraded,
            ComponentState::Healthy,
            ReadinessReason::Ready,
            "normal",
        ),
    ] {
        report_do_storage_health(&config, available, &health, &metrics).unwrap();
        assert_eq!(component().state, state, "{available} bytes");
        assert_eq!(component().reason, Some(reason), "{available} bytes");
        let rendered = metrics.render(&health.snapshot());
        for label in ["normal", "high", "stop"] {
            let value = u8::from(label == active_label);
            assert!(rendered.lines().any(|line| {
                line == format!("oc_do_storage_watermark{{state=\"{label}\"}} {value}")
            }));
        }
    }
}
