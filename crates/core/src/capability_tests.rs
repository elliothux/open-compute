use super::*;

fn unsupported_product() -> ProductCapabilityV1 {
    ProductCapabilityV1 {
        status: CapabilityStatus::Unsupported,
        kind: ProductKind::NonTarget,
        capability_version: None,
        members: Vec::new(),
        deviations: Vec::new(),
    }
}

fn platform_product(deviations: &[&str]) -> ProductCapabilityV1 {
    ProductCapabilityV1 {
        status: CapabilityStatus::SupportedWithDeviation,
        kind: ProductKind::Platform,
        capability_version: Some(1),
        members: Vec::new(),
        deviations: deviations.iter().map(|id| (*id).to_owned()).collect(),
    }
}

fn blocked_member(product: &str, symbol: &str, member: &str) -> CapabilityMemberV1 {
    CapabilityMemberV1 {
        id: format!("{product}::{symbol}::{member}:method#0"),
        product: product.to_owned(),
        symbol: symbol.to_owned(),
        member: member.to_owned(),
        kind: "method".to_owned(),
        overload: 0,
        readonly: false,
        optional: false,
        is_static: false,
        signature: format!("{member}(): void"),
        signature_sha256: "a".repeat(64),
        status: CapabilityStatus::Blocked,
        compile_cases: Vec::new(),
        runtime_cases: Vec::new(),
        deviations: Vec::new(),
    }
}

fn supported_member(status: CapabilityStatus) -> CapabilityMemberV1 {
    CapabilityMemberV1 {
        id: "workers::Socket::close:method#0".to_owned(),
        product: "workers".to_owned(),
        symbol: "Socket".to_owned(),
        member: "close".to_owned(),
        kind: "method".to_owned(),
        overload: 0,
        readonly: false,
        optional: false,
        is_static: false,
        signature: "close(): Promise<void>".to_owned(),
        signature_sha256: "b".repeat(64),
        status,
        compile_cases: vec!["raw-tcp-compile".to_owned()],
        runtime_cases: vec!["p0-2::raw-tcp".to_owned()],
        deviations: if status == CapabilityStatus::SupportedWithDeviation {
            vec!["OC-WKR-TCP-001".to_owned()]
        } else {
            Vec::new()
        },
    }
}

fn management_api() -> ManagementApiCapabilitiesV1 {
    ManagementApiCapabilitiesV1 {
        root: "/client/v4".to_owned(),
        routes: vec![ManagementApiRouteV1 {
            id: "GET /user".to_owned(),
            method: ManagementApiMethod::Get,
            path: "/user".to_owned(),
            status: InterfaceCapabilityStatus::Planned,
            source: "cloudflare-openapi".to_owned(),
            operation_id: Some("user-user-details".to_owned()),
            operation_sha256: Some("f".repeat(64)),
            request_media_type: ManagementApiRequestMediaType::None,
            stage: None,
            constraint: None,
            deviations: Vec::new(),
        }],
        legacy_routes: vec![LegacyManagementRouteV1 {
            id: "/operator/api/**".to_owned(),
            status: InterfaceCapabilityStatus::Unsupported,
            source: "day1-negative-route-inventory".to_owned(),
        }],
        deviations: Vec::new(),
    }
}

fn wrangler() -> WranglerCapabilitiesV1 {
    let item = WranglerCapabilityItemV1 {
        id: "name".to_owned(),
        status: InterfaceCapabilityStatus::Planned,
        source: "wrangler-config-schema".to_owned(),
        stage: None,
        constraint: None,
    };
    WranglerCapabilitiesV1 {
        version: "4.127.1".to_owned(),
        config_schema_sha256: "e".repeat(64),
        fields: vec![item.clone()],
        bindings: vec![WranglerCapabilityItemV1 {
            id: "plain_text".to_owned(),
            ..item.clone()
        }],
        commands: vec![WranglerCapabilityItemV1 {
            id: "deploy".to_owned(),
            ..item
        }],
    }
}

#[test]
fn capability_status_serialization_and_contract_are_strict() {
    assert_eq!(
        serde_json::to_string(&CapabilityStatus::Supported).unwrap(),
        "\"supported\""
    );
    assert_eq!(
        serde_json::to_string(&CapabilityStatus::SupportedWithDeviation).unwrap(),
        "\"supported_with_deviation\""
    );
    assert_eq!(
        serde_json::to_string(&CapabilityStatus::Blocked).unwrap(),
        "\"blocked\""
    );
    let release = PlatformReleaseIdentityV1 {
        schema_version: 1,
        platform_version: "0.1.0".to_owned(),
        git_revision: "test".to_owned(),
        rust_msrv: "1.98.0".to_owned(),
        workerd_version: "workerd test".to_owned(),
        workerd_lock_sha256: "a".repeat(64),
        runtime_assets_sha256: "b".repeat(64),
        dashboard_assets_sha256: "c".repeat(64),
        facade_capability_version: 1,
        control_schema_version: 8,
        scheduler_schema_version: 1,
        kv_schema_version: 1,
        d1_schema_version: 1,
        vectorize_schema_version: 1,
        ai_search_schema_version: 1,
        snapshot_format_version: 1,
    };
    let mut products = BTreeMap::new();
    for name in [
        "workers",
        "deployments",
        "static_assets",
        "service_bindings",
        "kv",
        "r2",
        "d1",
        "durable_objects",
        "alarms",
        "queues",
        "cron",
        "workflows",
        "workers_cache",
        "cache_api",
        "images",
        "version_metadata",
        "websocket_hibernation",
        "analytics_engine",
        "ai",
        "browser_rendering",
        "vectorize",
        "hyperdrive",
        "mtls",
        "rate_limiting",
        "workers_for_platforms",
    ] {
        products.insert(name.to_owned(), unsupported_product());
    }
    products.insert(
        "deployments".to_owned(),
        platform_product(&["OC-DEPLOY-001"]),
    );
    products.insert(
        "kv".to_owned(),
        ProductCapabilityV1 {
            status: CapabilityStatus::Blocked,
            kind: ProductKind::Target,
            capability_version: None,
            members: vec![blocked_member("kv", "KVNamespace", "get")],
            deviations: vec!["OC-KV-001".to_owned()],
        },
    );
    let mut capabilities = PlatformCapabilitiesV1 {
        schema_version: 1,
        release,
        runtime: RuntimeCapabilityV1 {
            effective_compatibility_date: "2026-08-30".to_owned(),
            workerd_lock_sha256: "a".repeat(64),
            workers_types_version: "5.20260830.1".to_owned(),
            workers_types_git_head: "e".repeat(40),
            workers_types_package_sha256: "c".repeat(64),
            workers_types_index_sha256: "e".repeat(64),
            workers_types_ast_sha256: "d".repeat(64),
        },
        products,
        management_api: management_api(),
        wrangler: wrangler(),
        limits: BTreeMap::new(),
    };
    assert!(capabilities.validate());

    capabilities.products.get_mut("queues").unwrap().kind = ProductKind::Target;
    capabilities.products.get_mut("queues").unwrap().status = CapabilityStatus::Blocked;
    capabilities
        .products
        .get_mut("queues")
        .unwrap()
        .capability_version = Some(1);
    assert!(!capabilities.validate());

    capabilities
        .products
        .get_mut("queues")
        .unwrap()
        .capability_version = None;
    capabilities.products.get_mut("queues").unwrap().members =
        vec![blocked_member("queues", "Queue", "send")];
    assert!(capabilities.validate());
    capabilities.products.get_mut("queues").unwrap().members[0].status =
        CapabilityStatus::Supported;
    assert!(!capabilities.validate());
    capabilities.products.get_mut("queues").unwrap().members[0].status = CapabilityStatus::Blocked;
    capabilities.products.get_mut("queues").unwrap().members[0]
        .compile_cases
        .push("p0-4::example".to_owned());
    assert!(!capabilities.validate());
    assert!(
        serde_json::from_str::<ProductCapabilityV1>(
            r#"{"status":"unsupported","kind":"non_target","unknown":true}"#
        )
        .is_err()
    );
    assert!(
        serde_json::from_str::<ProductCapabilityV1>(
            r#"{"status":"unsupported","kind":"non_target","methods":["send"]}"#
        )
        .is_err()
    );
}

#[test]
fn member_and_product_status_combinations_validate_exactly() {
    for status in [
        CapabilityStatus::Supported,
        CapabilityStatus::SupportedWithDeviation,
    ] {
        assert!(supported_member(status).validate());
    }
    let mut invalid = supported_member(CapabilityStatus::Supported);
    invalid.compile_cases.clear();
    assert!(!invalid.validate());
    invalid = supported_member(CapabilityStatus::SupportedWithDeviation);
    invalid.deviations.clear();
    assert!(!invalid.validate());
    invalid = supported_member(CapabilityStatus::Supported);
    invalid.signature_sha256 = "not-a-digest".to_owned();
    assert!(!invalid.validate());

    let supported = ProductCapabilityV1 {
        status: CapabilityStatus::Supported,
        kind: ProductKind::Target,
        capability_version: Some(1),
        members: vec![supported_member(CapabilityStatus::Supported)],
        deviations: Vec::new(),
    };
    assert!(supported.validate());
    let deviating = ProductCapabilityV1 {
        status: CapabilityStatus::SupportedWithDeviation,
        kind: ProductKind::Target,
        capability_version: Some(1),
        members: vec![supported_member(CapabilityStatus::SupportedWithDeviation)],
        deviations: vec!["OC-WKR-TCP-001".to_owned()],
    };
    assert!(deviating.validate());
    let platform = ProductCapabilityV1 {
        status: CapabilityStatus::Supported,
        kind: ProductKind::Platform,
        capability_version: Some(1),
        members: Vec::new(),
        deviations: Vec::new(),
    };
    assert!(platform.validate());
    assert!(unsupported_product().validate());
}
