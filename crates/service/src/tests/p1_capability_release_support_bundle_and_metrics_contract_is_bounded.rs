use super::*;

#[tokio::test]
async fn p1_capability_release_support_bundle_and_metrics_contract_is_bounded() {
    assert_eq!(
        crate::snapshot_pins::SnapshotPins::Unavailable
            .contains_object_key("system/artifacts/v1/sha256/untrusted")
            .unwrap_err()
            .code(),
        ErrorCode::ResourceReferenced
    );
    let (dir, path, _mock) = initialized_doctor_fixture().await;
    let mut loaded = load_fixture_platform_config(&path);
    let capabilities = crate::capabilities::platform_capabilities(&loaded.config).unwrap();
    assert!(capabilities.validate());
    assert!(
        capabilities.products["durable_objects"]
            .members
            .iter()
            .any(|member| member.member == "get"
                && member.status != open_compute_core::CapabilityStatus::Blocked)
    );
    assert_eq!(
        capabilities.products["queues"].status,
        open_compute_core::CapabilityStatus::SupportedWithDeviation
    );
    assert_eq!(
        capabilities.products["queues"].deviations,
        vec!["OC-QUEUE-001"]
    );
    assert_eq!(
        capabilities.products["cron"].status,
        open_compute_core::CapabilityStatus::SupportedWithDeviation
    );
    assert_eq!(
        capabilities.products["cron"].deviations,
        vec!["OC-CRON-001"]
    );
    assert_eq!(
        capabilities.products["workflows"].status,
        open_compute_core::CapabilityStatus::SupportedWithDeviation
    );
    assert_eq!(
        capabilities.products["workflows"].deviations,
        vec!["OC-WORKFLOW-001"]
    );
    assert_eq!(
        capabilities.products["websocket_hibernation"].status,
        open_compute_core::CapabilityStatus::Supported
    );
    assert!(!capabilities.runtime.workers_types_version.is_empty());
    assert_eq!(capabilities.runtime.workers_types_ast_sha256.len(), 64);
    let metadata = crate::capabilities::platform_release_metadata(&loaded).unwrap();
    assert!(metadata.validate());
    assert_eq!(metadata.release, capabilities.release);
    assert_eq!(
        metadata.schema_definitions.last().unwrap().version,
        metadata.release.control_schema_version
    );
    let policy = crate::capabilities::platform_config_policy_sha256(&loaded).unwrap();
    let original_data_dir = loaded.config.data.path.clone();
    let original_master_key_file = loaded.config.data.master_key_file.clone();
    let original_public_bind = loaded.config.server.public_bind;
    let original_admin_bind = loaded.config.server.admin_bind;
    loaded.config.data.path = dir.path().join("relocated-data");
    loaded.config.data.master_key_file = dir.path().join("relocated-recovery-key");
    loaded.config.server.public_bind = "127.0.0.1:65001".parse().unwrap();
    loaded.config.server.admin_bind = Some("127.0.0.1:65002".to_owned());
    assert_eq!(
        crate::capabilities::platform_config_policy_sha256(&loaded).unwrap(),
        policy,
        "host paths and listener ports are intentionally outside restore policy"
    );
    loaded.config.kv.namespace_quota_bytes += 4096;
    assert_ne!(
        crate::capabilities::platform_config_policy_sha256(&loaded).unwrap(),
        policy,
        "product semantics must change the authenticated restore policy"
    );
    loaded.config.kv.namespace_quota_bytes -= 4096;
    loaded.config.data.path = original_data_dir;
    loaded.config.data.master_key_file = original_master_key_file;
    loaded.config.server.public_bind = original_public_bind;
    loaded.config.server.admin_bind = original_admin_bind;

    let operations = loaded.config.data.path.join("operations");
    fs::create_dir(&operations).unwrap();
    fs::set_permissions(&operations, fs::Permissions::from_mode(0o700)).unwrap();
    let snapshot_receipt = operations.join("last-snapshot.json");
    write_mode(
        &snapshot_receipt,
        r#"{"schema_version":1,"created_at_ms":1,"verified":true}"#,
        0o600,
    );
    let outside_receipt = dir.path().join("outside-receipt.json");
    write_mode(&outside_receipt, r#"{"secret":"outside"}"#, 0o600);
    std::os::unix::fs::symlink(&outside_receipt, operations.join("last-restore.json")).unwrap();

    let output = fs::canonicalize(dir.path())
        .unwrap()
        .join("open-compute-support.tar");
    let result = crate::support_bundle::create_support_bundle(&loaded, &output)
        .await
        .unwrap();
    assert_eq!(result.entries, 10);
    assert_eq!(
        fs::metadata(&output).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let archive = fs::read(output).unwrap();
    assert!(!archive.windows(4).any(|window| window == b"AKIA"));
    assert!(
        !archive
            .windows(b"wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY".len())
            .any(|window| window == b"wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY")
    );
    for name in [
        b"config-policy.json".as_slice(),
        b"doctor.json".as_slice(),
        b"metrics.prom".as_slice(),
        b"object-storage.json".as_slice(),
        b"receipts/last-snapshot.json".as_slice(),
        b"release.json".as_slice(),
        b"search.json".as_slice(),
    ] {
        assert!(archive.windows(name.len()).any(|window| window == name));
    }
    if let open_compute_core::ObjectStorageConfig::Local(local) = &loaded.config.object_storage {
        let path = local.path.as_os_str().as_encoded_bytes();
        assert!(!archive.windows(path.len()).any(|window| window == path));
    }
    assert!(archive.windows(64).any(|window| {
        window
            .iter()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
    }));
    let search = crate::support_bundle::search_summary(&loaded).unwrap();
    assert_eq!(search["schema_version"], 2);
    assert_eq!(search["resources"]["vectorize_index"]["total"], 0);
    assert_eq!(search["resources"]["ai_search_namespace"]["total"], 0);
    assert_eq!(search["resources"]["ai_search_instance"]["total"], 0);
    assert_eq!(
        search["contracts"]["ai_backend_catalog_sha256"]
            .as_str()
            .unwrap()
            .len(),
        64
    );
    let search_json = serde_json::to_string(&search).unwrap();
    for forbidden in ["metadata", "values", "object_key", "secret"] {
        assert!(!search_json.contains(forbidden));
    }
    assert_eq!(
        crate::support_bundle::create_support_bundle(&loaded, Path::new("relative.tar"))
            .await
            .unwrap_err()
            .code(),
        ErrorCode::SupportBundleInvalid
    );
    let existing = fs::canonicalize(dir.path())
        .unwrap()
        .join("existing-support.tar");
    fs::write(&existing, b"existing").unwrap();
    assert_eq!(
        crate::support_bundle::create_support_bundle(&loaded, &existing)
            .await
            .unwrap_err()
            .code(),
        ErrorCode::SupportBundleInvalid
    );
    loaded.config.hardening.max_support_bundle_bytes = 1;
    assert_eq!(
        crate::support_bundle::create_support_bundle(
            &loaded,
            &fs::canonicalize(dir.path())
                .unwrap()
                .join("bounded-support.tar"),
        )
        .await
        .unwrap_err()
        .code(),
        ErrorCode::SupportBundleInvalid
    );
    loaded.config.hardening.max_support_bundle_bytes = 32 * 1024 * 1024;

    fs::remove_file(operations.join("last-restore.json")).unwrap();
    write_mode(&operations.join("last-restore.json"), "not-json", 0o600);
    assert_eq!(
        crate::support_bundle::create_support_bundle(
            &loaded,
            &fs::canonicalize(dir.path())
                .unwrap()
                .join("invalid-receipt-support.tar"),
        )
        .await
        .unwrap_err()
        .code(),
        ErrorCode::SupportBundleInvalid
    );
    fs::remove_file(operations.join("last-restore.json")).unwrap();

    let admin_secret = dir.path().join("admin-auth-secret");
    write_mode(&admin_secret, "p1-support-admin-secret", 0o600);
    loaded.config.server.admin_auth = SecretReference {
        env: None,
        file: Some(admin_secret),
    };
    let admin_output = fs::canonicalize(dir.path())
        .unwrap()
        .join("admin-support.tar");
    crate::support_bundle::create_support_bundle(&loaded, &admin_output)
        .await
        .unwrap();
    assert!(
        !fs::read(admin_output)
            .unwrap()
            .windows(b"p1-support-admin-secret".len())
            .any(|window| window == b"p1-support-admin-secret")
    );

    assert_metrics(&loaded.config.metrics);
}
fn assert_metrics(config: &MetricsConfig) {
    let metrics = MetricsRegistry::new(config, "test", "workerd").unwrap();
    metrics.observe_admission(open_compute_core::OperationClass::Kv, None);
    metrics.observe_admission(
        open_compute_core::OperationClass::Workers,
        Some(ErrorCode::QuotaExceeded),
    );
    metrics.observe_admission(
        open_compute_core::OperationClass::D1,
        Some(ErrorCode::AdmissionBusy),
    );
    metrics.observe_admission(
        open_compute_core::OperationClass::Restore,
        Some(ErrorCode::StoragePressure),
    );
    metrics.observe_admission(
        open_compute_core::OperationClass::Snapshot,
        Some(ErrorCode::PlatformUnavailable),
    );
    metrics.set_disk_admission(
        &open_compute_core::AdmissionSnapshotV1 {
            schema_version: 1,
            filesystem_free_bytes: 100,
            soft_reserve_bytes: 80,
            hard_reserve_bytes: 60,
            emergency_reserve_bytes: 10,
            reserved_bytes: 7,
            owned_staging_bytes: 3,
            mode: open_compute_core::PlatformMode::Serving,
        },
        10,
    );
    metrics.set_schema_version(8);
    metrics.set_schema_failed_resources(2);
    metrics.set_resource_counts([1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11]);
    metrics.observe_product_error(
        open_compute_core::OperationClass::DurableObjects,
        ErrorCode::QuotaExceeded,
    );
    metrics.inc_sqlite_busy();
    metrics.inc_sqlite_check_failure();
    metrics.inc_websocket_close(WebSocketCloseReason::VersionRestart);
    for reason in [
        WebSocketCloseReason::Normal,
        WebSocketCloseReason::Shutdown,
        WebSocketCloseReason::Error,
        WebSocketCloseReason::Disconnected,
    ] {
        metrics.inc_websocket_close(reason);
    }
    metrics.record_snapshot_receipt(122, Duration::from_millis(11));
    metrics.record_snapshot_receipt_at(123, Duration::from_millis(12), 1);
    metrics.record_restore_receipt(1, Duration::from_millis(34), true);
    metrics
        .set_release_identity(&"a".repeat(64), "p1.0-capabilities-v1")
        .unwrap();
    assert!(metrics.set_release_identity("BAD", "p1").is_err());
    metrics.inc_quota_reject("unsupported-product");
    metrics.observe_product_error(
        open_compute_core::OperationClass::Scheduler,
        ErrorCode::QuotaExceeded,
    );
    metrics.observe_product_error(open_compute_core::OperationClass::Kv, ErrorCode::KvBusy);
    metrics.observe_product_error(
        open_compute_core::OperationClass::D1,
        ErrorCode::D1Overloaded,
    );
    let mut staging_gauge = KvStagingGauge::new(None);
    staging_gauge.add(5);
    assert!(format!("{staging_gauge:?}").contains("bytes: 5"));
    for operation in [
        ResourceOperation::Create,
        ResourceOperation::Get,
        ResourceOperation::List,
        ResourceOperation::Rename,
        ResourceOperation::Delete,
    ] {
        metrics.observe_resource_operation(operation, false, Duration::from_millis(1));
        metrics.observe_resource_operation(operation, true, Duration::from_millis(2));
    }
    metrics.set_resource_open_handles(7);
    metrics.observe_resource_pin_wait(Duration::from_millis(3));
    for deleting in [false, true] {
        for success in [false, true] {
            metrics.inc_resource_reconcile(deleting, success);
        }
    }
    let rendered = metrics.render(&PlatformStatus::starting());
    assert!(rendered.contains("platform_admission_total{operation=\"kv\",outcome=\"accepted\"} 1"));
    assert!(rendered.contains(
        "platform_admission_total{operation=\"restore\",outcome=\"storage_pressure\"} 1"
    ));
    assert!(rendered.contains("platform_schema_current 8"));
    assert!(rendered.contains("platform_schema_failed_resources 2"));
    assert!(rendered.contains("platform_resource_count{resource=\"d1_databases\"} 7"));
    assert!(rendered.contains("platform_resource_count{resource=\"vectorize_indexes\"} 9"));
    assert!(rendered.contains("platform_resource_count{resource=\"ai_search_namespaces\"} 10"));
    assert!(rendered.contains("platform_resource_count{resource=\"ai_search_instances\"} 11"));
    assert!(rendered.contains("platform_quota_reject_total{product=\"durable_objects\"} 1"));
    assert!(rendered.contains("sqlite_busy_total 3"));
    assert!(rendered.contains("sqlite_check_failure_total 1"));
    assert!(rendered.contains("oc_do_websocket_close_total{reason=\"version_restart\"} 1"));
    assert!(rendered.contains("platform_restore_last_smoke_verified 1"));
    assert!(rendered.contains("conformance_result=\"p1.0-capabilities-v1\""));
    assert!(rendered.contains(
    "resource_operations_total{kind=\"kv_namespace\",operation=\"create\",outcome=\"success\"} 1"
));
    assert!(rendered.contains("resource_open_handles{kind=\"kv_namespace\"} 7"));
}
