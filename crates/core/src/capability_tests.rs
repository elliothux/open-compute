use super::*;

#[test]
fn capability_status_serialization_and_contract_are_strict() {
    assert_eq!(
        serde_json::to_string(&CapabilityStatus::Supported).unwrap(),
        "\"supported\""
    );
    let product = ProductCapabilityV1 {
        status: CapabilityStatus::Unsupported,
        capability_version: None,
        methods: Vec::new(),
        deviations: Vec::new(),
        basic_websocket: None,
        hibernatable_websocket: None,
    };
    let release = PlatformReleaseIdentityV1 {
        schema_version: 1,
        platform_version: "0.1.0".to_owned(),
        git_revision: "test".to_owned(),
        rust_msrv: "1.98.0".to_owned(),
        workerd_version: "workerd test".to_owned(),
        workerd_lock_sha256: "a".repeat(64),
        runtime_assets_sha256: "b".repeat(64),
        facade_capability_version: 1,
        control_schema_version: 8,
        scheduler_schema_version: 1,
        kv_schema_version: 1,
        d1_schema_version: 1,
        snapshot_format_version: 1,
        compatibility_policy_sha256: "c".repeat(64),
    };
    let mut products = BTreeMap::new();
    for name in [
        "workers",
        "kv",
        "r2",
        "d1",
        "durable_objects",
        "alarms",
        "queues",
        "cron",
        "workflows",
        "websocket_hibernation",
    ] {
        products.insert(name.to_owned(), product.clone());
    }
    let mut capabilities = PlatformCapabilitiesV1 {
        schema_version: 1,
        release,
        runtime: RuntimeCapabilityV1 {
            compatibility_date_min: "2026-01-01".to_owned(),
            compatibility_date_max: "2026-12-31".to_owned(),
            allowed_flags: Vec::new(),
            denied_flags: Vec::new(),
            workerd_lock_sha256: "a".repeat(64),
        },
        products,
        limits: BTreeMap::new(),
    };
    assert!(capabilities.validate());
    capabilities
        .products
        .get_mut("queues")
        .unwrap()
        .capability_version = Some(1);
    assert!(!capabilities.validate());
}
