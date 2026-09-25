use super::*;

#[tokio::test]
async fn p1_startup_receipts_health_and_inventory_metrics_cover_real_authority() {
    let (_dir, path, _mock) = initialized_doctor_fixture().await;
    let loaded = load_fixture_platform_config(&path);
    let data_dir = DataDir::acquire_existing_offline(&loaded.config.data).unwrap();
    let now_ms = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    data_dir
        .write_operation_receipt(
            "last-snapshot.json",
            serde_json::to_vec(&serde_json::json!({
                "bytes": 321,
                "created_at_ms": now_ms,
                "duration_ms": 12,
                "verified": true
            }))
            .unwrap()
            .as_slice(),
        )
        .unwrap();
    data_dir
        .write_operation_receipt(
            "last-restore.json",
            serde_json::to_vec(&serde_json::json!({
                "restored_at_ms": now_ms,
                "duration_ms": 34,
                "smoke_verified": true
            }))
            .unwrap()
            .as_slice(),
        )
        .unwrap();

    let metrics = MetricsRegistry::new(&loaded.config.metrics, "test", "workerd").unwrap();
    crate::run::p1::load_offline_metrics_receipts(&data_dir, &metrics);
    let health = HealthCoordinator::new();
    crate::run::p1::update_operations_health(&data_dir, 60_000, &health).unwrap();
    let operations = health
        .snapshot()
        .components
        .into_iter()
        .find(|component| component.name == ComponentName::Operations)
        .unwrap();
    assert_eq!(operations.state, ComponentState::Healthy);

    let storage = open_compute_storage::PlatformStorage::bootstrap(
        &loaded.config.data,
        &open_compute_core::SystemClock,
    );
    assert_eq!(storage.unwrap_err().code(), ErrorCode::DataDirInUse);
    drop(data_dir);
    let storage = open_compute_storage::PlatformStorage::bootstrap(
        &loaded.config.data,
        &open_compute_core::SystemClock,
    )
    .unwrap();
    crate::run::p1::refresh_metrics(
        &storage,
        &metrics,
        loaded.config.hardening.emergency_reserve_bytes,
    )
    .unwrap();
    let rendered = metrics.render(&health.snapshot());
    assert!(rendered.contains("platform_snapshot_last_bytes 321"));
    assert!(rendered.contains("platform_restore_last_smoke_verified 1"));
    assert!(rendered.contains("platform_resource_count{resource=\"instances\"} 1"));
    drop(storage);

    let data_dir = DataDir::acquire_existing_offline(&loaded.config.data).unwrap();
    data_dir
        .write_operation_receipt("last-snapshot.json", br#"{"verified":false}"#)
        .unwrap();
    crate::run::p1::load_offline_metrics_receipts(&data_dir, &metrics);
    crate::run::p1::update_operations_health(&data_dir, 0, &health).unwrap();
    let operations = health
        .snapshot()
        .components
        .into_iter()
        .find(|component| component.name == ComponentName::Operations)
        .unwrap();
    assert_eq!(operations.state, ComponentState::Degraded);
    assert_eq!(operations.reason, Some(ReadinessReason::SnapshotStale));
}
