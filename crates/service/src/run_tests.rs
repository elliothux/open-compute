use super::*;
use crate::instance_registry::{InstanceRecord, ServiceScope};

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
    // the assertions do not depend on the host's actual filesystem usage.
    let clear = open_compute_core::DurableObjectsConfig {
        disk_high_watermark_percent: 100,
        ..open_compute_core::DurableObjectsConfig::default()
    };
    update_do_storage_health(&storage, &clear, &health, &metrics).unwrap();
    assert_eq!(component().state, ComponentState::Healthy);
    assert_eq!(component().reason, Some(ReadinessReason::Ready));

    // A soft watermark below real usage reports the soft limit while the stop-writes
    // threshold stays above it.
    let soft = open_compute_core::DurableObjectsConfig {
        disk_high_watermark_percent: 1,
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

fn record(config: &std::path::Path) -> InstanceRecord {
    let id = open_compute_core::InstanceId::from_canonical_config_path(config).unwrap();
    let digest = open_compute_core::digest_canonical_config_path(config).unwrap();
    InstanceRecord {
        schema_version: 1,
        instance_id: id.as_str().to_owned(),
        digest_sha256: hex::encode(digest),
        canonical_config_path: config.to_string_lossy().into_owned(),
        config_sha256: "0".repeat(64),
        data_path: String::new(),
        object_authority: crate::instance_registry::RegisteredObjectAuthority::Local {
            path: String::new(),
        },
        binary_path: String::new(),
        service_scope: ServiceScope::User,
        service_user: None,
        service_identifier: String::new(),
        created_at: 0,
    }
}

#[test]
fn control_identity_prefers_the_single_matching_registration() {
    let root = tempfile::tempdir().unwrap();
    let config = root.path().join("open-compute.toml");
    let first = record(&config);
    let expected = first.instance_id().unwrap();
    let second = record(&root.path().join("other.toml"));
    assert_eq!(
        control_identity_from_records(&config, vec![first, second]).unwrap(),
        (expected, ServiceScope::User)
    );
}

#[test]
fn control_identity_rejects_duplicate_registrations_for_one_config() {
    let root = tempfile::tempdir().unwrap();
    let config = root.path().join("dup.toml");
    let first = record(&config);
    let second = record(&config);
    assert_eq!(
        control_identity_from_records(&config, vec![first, second])
            .unwrap_err()
            .code(),
        ErrorCode::InstanceRegistryInvalid
    );
}

#[test]
fn control_identity_falls_back_to_the_canonical_config_path_identity() {
    let root = tempfile::tempdir().unwrap();
    let config = root.path().join("fallback.toml");
    let (id, scope) = control_identity_from_records(&config, Vec::new()).unwrap();
    assert_eq!(
        id,
        open_compute_core::InstanceId::from_canonical_config_path(&config).unwrap()
    );
    assert_eq!(scope, ServiceScope::User);
    let system = std::path::Path::new("/etc/open-compute/ocd.toml");
    let (_, scope) = control_identity_from_records(system, Vec::new()).unwrap();
    assert_eq!(scope, ServiceScope::System);
}
