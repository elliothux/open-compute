use super::*;

#[test]
fn snapshot_policy_covers_the_current_workflow_configuration() {
    let mut loaded = LoadedConfig {
        path: "/unused/policy.toml".into(),
        config: PlatformConfig::default(),
    };
    let initial = platform_config_policy_sha256(&loaded).unwrap();
    loaded.config.workflows.max_steps = 512;
    assert_ne!(platform_config_policy_sha256(&loaded).unwrap(), initial);
    loaded.config.workflows = open_compute_core::WorkflowsConfig::default();
    assert_eq!(platform_config_policy_sha256(&loaded).unwrap(), initial);
    loaded.config.response_cache.max_object_bytes /= 2;
    assert_ne!(platform_config_policy_sha256(&loaded).unwrap(), initial);
    loaded.config.response_cache = open_compute_core::ResponseCacheConfig::default();
    loaded.config.images.max_concurrency = 2;
    assert_ne!(platform_config_policy_sha256(&loaded).unwrap(), initial);
    loaded.config.images = open_compute_core::ImagesConfig::default();
    assert_eq!(platform_config_policy_sha256(&loaded).unwrap(), initial);
    for change in [
        |config: &mut open_compute_core::WorkflowsConfig| config.max_parallel_steps = 2,
        |config: &mut open_compute_core::WorkflowsConfig| config.max_buffered_events = 64,
        |config: &mut open_compute_core::WorkflowsConfig| config.max_event_bytes = 1_048_576,
        |config: &mut open_compute_core::WorkflowsConfig| {
            config.default_retention.success_retention_ms = 3_600_000;
        },
        |config: &mut open_compute_core::WorkflowsConfig| {
            config.default_retention.error_retention_ms = 7_200_000;
        },
    ] {
        change(&mut loaded.config.workflows);
        loaded.config.validate().unwrap();
        assert_ne!(platform_config_policy_sha256(&loaded).unwrap(), initial);
        loaded.config.workflows = open_compute_core::WorkflowsConfig::default();
        assert_eq!(platform_config_policy_sha256(&loaded).unwrap(), initial);
    }
}

#[test]
fn workflow_capabilities_report_current_model_and_operator_limits() {
    let products = product_registry();
    for product in products.values() {
        if product.status == CapabilityStatus::Supported {
            assert_eq!(product.capability_version, Some(1));
        }
    }
    assert!(
        products["workflows"]
            .methods
            .iter()
            .any(|method| method == "step.waitForEvent")
    );
    assert_eq!(
        products["cache_api"].deviations,
        ["OC-CACHE-001", "OC-CACHE-002"]
    );
    assert_eq!(products["images"].deviations, ["OC-IMAGES-001"]);
    let mut loaded = LoadedConfig {
        path: "/unused/policy.toml".into(),
        config: PlatformConfig::from_toml_str("").unwrap(),
    };
    loaded.config.workflows.max_parallel_steps = 2;
    loaded.config.workflows.max_buffered_events = 8;
    loaded.config.workflows.max_event_bytes = 1_048_576;
    loaded.config.workflows.dispatch_timeout_ms = 90_000;
    loaded
        .config
        .workflows
        .default_retention
        .success_retention_ms = 3_600_000;
    loaded.config.workflows.default_retention.error_retention_ms = 7_200_000;
    let limits = limit_registry(&loaded.config);
    assert_eq!(
        limits["response_cache.max_object_bytes"],
        loaded.config.response_cache.max_object_bytes
    );
    assert_eq!(limits["images.max_input_bytes"], 20 * 1024 * 1024);
    for (name, expected) in [
        ("max_parallel_steps", 2),
        ("max_buffered_events", 8),
        ("max_event_bytes", 1_048_576),
        ("max_attempt_ms", 60_000),
        ("default_success_retention_ms", 3_600_000),
        ("default_error_retention_ms", 7_200_000),
    ] {
        assert_eq!(limits[&format!("workflows.{name}")], expected);
    }
}
