use super::*;
use open_compute_core::{CapabilityStatus, ProductKind};

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
    let inventory: CapabilityInventoryV1 = serde_json::from_slice(include_bytes!(
        "../../../share/cloudflare-capabilities.json"
    ))
    .unwrap();
    assert!(inventory.management_api.validate());
    assert!(inventory.wrangler.validate());
    assert!(inventory.source.validate());
    for (name, product) in &inventory.products {
        assert!(product.validate(), "invalid product: {name}");
    }
    assert!(inventory.validate());
    let (source, products, management_api, workers_observability, wrangler) =
        product_registry().unwrap();
    assert!(!source.workers_types_version.is_empty());
    assert_eq!(source.ast_sha256.len(), 64);
    assert!(management_api.validate());
    assert!(workers_observability.validate());
    assert_eq!(workers_observability.script_tail_protocol, "trace-v1");
    assert!(wrangler.validate());
    for product in products.values() {
        if matches!(
            product.status,
            CapabilityStatus::Supported | CapabilityStatus::SupportedWithDeviation
        ) {
            assert_eq!(product.capability_version, Some(1));
        }
        if product.kind == ProductKind::Target {
            assert!(matches!(
                product.status,
                CapabilityStatus::Supported | CapabilityStatus::SupportedWithDeviation
            ));
            assert!(!product.members.is_empty());
            assert!(
                product
                    .members
                    .iter()
                    .all(open_compute_core::CapabilityMemberV1::validate)
            );
            assert!(
                product
                    .members
                    .iter()
                    .all(|member| member.status != CapabilityStatus::Blocked)
            );
        }
    }
    assert!(
        products["workflows"]
            .members
            .iter()
            .any(|member| { member.member == "waitForEvent" || member.member == "do" })
    );
    assert_eq!(
        products["cache_api"].deviations,
        ["OC-CACHE-001", "OC-CACHE-002"]
    );
    assert_eq!(products["images"].deviations, ["OC-IMAGES-001"]);
    assert_eq!(
        products["vectorize"].status,
        CapabilityStatus::SupportedWithDeviation
    );
    assert_eq!(products["vectorize"].deviations, ["OC-VECTORIZE-001"]);
    assert!(
        products["vectorize"]
            .members
            .iter()
            .any(|member| member.symbol == "Vectorize" && member.member == "queryById")
    );
    assert_eq!(
        products["ai"].deviations,
        ["OC-AI-MARKDOWN-001", "OC-AI-SEARCH-001"]
    );
    assert_eq!(
        products["service_bindings"].status,
        CapabilityStatus::SupportedWithDeviation
    );
    assert_eq!(
        products["websocket_hibernation"].status,
        CapabilityStatus::Supported
    );
    assert!(
        products["websocket_hibernation"]
            .members
            .iter()
            .any(|member| {
                member.symbol == "DurableObjectState" && member.member == "acceptWebSocket"
            })
    );
    assert!(
        products["d1"].members.iter().any(|member| {
            member.symbol == "D1DatabaseSession" && member.member == "getBookmark"
        })
    );
    assert!(
        products["queues"].members.iter().any(|member| {
            member.symbol == "QueueSendOptions" && member.member == "contentType"
        })
    );
    assert!(
        products["workflows"]
            .members
            .iter()
            .any(|member| { member.member == "createBatch" })
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
