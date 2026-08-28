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
}

#[test]
fn workflow_capabilities_report_explicit_v2_and_current_operator_limits() {
    let products = product_registry();
    for (name, product) in &products {
        if product.status == CapabilityStatus::Supported {
            assert_eq!(
                product.capability_version,
                Some(if name == "workflows" { 2 } else { 1 })
            );
        }
    }
    assert!(
        products["workflows"]
            .methods
            .iter()
            .any(|method| method == "step.waitForEvent")
    );
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
