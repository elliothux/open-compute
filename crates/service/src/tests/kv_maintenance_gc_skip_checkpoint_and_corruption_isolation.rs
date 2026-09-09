use super::*;

#[tokio::test]
async fn kv_maintenance_gc_skip_checkpoint_and_corruption_isolation() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("data");
    let storage = Arc::new(
        open_compute_storage::PlatformStorage::bootstrap(
            &open_compute_core::DataConfig {
                path: root.clone(),
                master_key_file: root.join("keys/master.key"),
                master_key_env: None,
                sqlite_busy_timeout_ms: 5_000,
                free_space_soft_bytes: 1_073_741_824,
                free_space_hard_bytes: 268_435_456,
            },
            &open_compute_core::SystemClock,
        )
        .unwrap(),
    );
    let account = storage.identity().default_account_id;
    let pins = open_compute_workers::ResourcePins::new();
    let created = open_compute_workers::ResourceController::new(
        &storage,
        pins.clone(),
        open_compute_workers::KvResourceDriver::new(&storage, 256 * 1024 * 1024),
    )
    .create(&open_compute_workers::CreateResourceRequest {
        account_id: account,
        kind: open_compute_core::BindingKind::KvNamespace,
        name: "maintenance".to_owned(),
        idempotency_key: "maintenance-create".to_owned(),
        driver_schema_version: 1,
        request_id: open_compute_core::RequestId::generate(),
        now_ms: 1,
    })
    .unwrap();
    let resource = match created {
        open_compute_workers::CreateResourceOutcome::Applied(value) => value.resource_id,
        open_compute_workers::CreateResourceOutcome::Replay(_) => unreachable!(),
    };
    let catalog = open_compute_storage::KvNamespaceRepository::new(storage.db());
    let record = catalog.get(account, resource).unwrap();
    let database = open_compute_storage::KvPaths::open(storage.data_dir().root())
        .unwrap()
        .resolve_storage_key(&record.storage_key, account, resource)
        .unwrap();
    let engine = open_compute_storage::KvEngine::from_record(database.clone(), &record).unwrap();
    engine
        .put(
            "expired",
            b"value",
            &open_compute_storage::KvPutOptions {
                expires_at_ms: Some(60_001),
                metadata_json: None,
            },
            1,
        )
        .unwrap();
    let metrics =
        Arc::new(MetricsRegistry::new(&MetricsConfig::default(), "test", "workerd").unwrap());
    let pin = pins.try_pin(resource).unwrap();
    run_kv_maintenance(
        &storage,
        &pins,
        &open_compute_core::KvConfig::default(),
        &metrics,
    )
    .await;
    assert!(engine.get("expired", 1).unwrap().is_some());
    drop(pin);
    run_kv_maintenance(
        &storage,
        &pins,
        &open_compute_core::KvConfig::default(),
        &metrics,
    )
    .await;
    assert!(engine.get("expired", i64::MAX).unwrap().is_none());
    let conn = rusqlite::Connection::open(database).unwrap();
    conn.execute(
        "UPDATE kv_meta SET value = ?1 WHERE key = 'resource_id'",
        [b"wrong".as_slice()],
    )
    .unwrap();
    drop(conn);
    run_kv_maintenance(
        &storage,
        &pins,
        &open_compute_core::KvConfig::default(),
        &metrics,
    )
    .await;
    let isolated = open_compute_storage::ResourceRepository::new(storage.db())
        .get(account, resource)
        .unwrap();
    assert_eq!(
        isolated.availability,
        open_compute_core::ResourceAvailability::Unavailable
    );
    let rendered = metrics.render(&PlatformStatus::starting());
    assert!(rendered.contains("kv_gc_entries_total{outcome=\"success\"} 1"));
    assert!(rendered.contains("kv_checkpoint_total{outcome=\"success\"} 1"));
    assert!(rendered.contains("kv_corruption_total{class=\"sqlite\"} 1"));
}
