use super::*;

#[test]
fn p1_schema_inspection_checks_current_kv_and_d1_files_without_mutation() {
    let (_tmp, root) = unique_root();
    let config = storage_config(&root);
    let storage = PlatformStorage::bootstrap(&config, &SystemClock).unwrap();
    let scheduler_path = storage.data_dir().ensure_scheduler_db().unwrap();
    drop(crate::SchedulerStore::open(&scheduler_path, 5_000, 1).unwrap());
    let account = storage.identity().default_account_id;

    let reserve = |kind, name: &str, key: &str| {
        let fingerprint = storage.crypto().fingerprint_request(key.as_bytes());
        let reserved = ResourceRepository::new(storage.db())
            .reserve_create(
                &ReserveResourceCreate {
                    account_id: account,
                    kind,
                    name,
                    idempotency_key: key,
                    fingerprint_key_id: storage.crypto().fingerprint_key_id(),
                    request_fingerprint: &fingerprint,
                    resource_id: ResourceId::generate(),
                    driver_schema_version: 1,
                    request_id: open_compute_core::RequestId::generate(),
                    now_ms: 1,
                    expires_at_ms: 10,
                },
                1_000_000,
            )
            .unwrap();
        let ResourceCreateReservation::Reserved(resource) = reserved else {
            panic!("resource must be newly reserved");
        };
        resource
    };

    let kv = reserve(BindingKind::KvNamespace, "schema-kv", "schema-kv");
    let kv_paths = crate::KvPaths::open(&root).unwrap();
    let kv_key = crate::KvPaths::storage_key(account, kv.id);
    crate::KvNamespaceRepository::new(storage.db())
        .ensure_namespace(&kv, &kv_key, crate::KV_SCHEMA_VERSION, 256 * 1024 * 1024)
        .unwrap();
    let kv_staging = kv_paths.create_namespace_staging(kv.id).unwrap();
    drop(
        crate::KvEngine::create(
            &kv_staging.join("data.sqlite"),
            account,
            kv.id,
            1,
            256 * 1024 * 1024,
        )
        .unwrap(),
    );
    kv_paths
        .publish_staging(&kv_staging, account, kv.id)
        .unwrap();

    let d1 = reserve(BindingKind::D1Database, "schema-d1", "schema-d1");
    let d1_paths = crate::D1Paths::open(&root).unwrap();
    let d1_key = crate::D1Paths::storage_key(account, d1.id);
    crate::D1DatabaseRepository::new(storage.db())
        .ensure_database(
            &d1,
            &d1_key,
            crate::D1_DATABASE_SCHEMA_VERSION,
            64 * 1024 * 1024,
        )
        .unwrap();
    let d1_staging = d1_paths.create_database_staging(d1.id).unwrap();
    drop(
        crate::D1Engine::create(
            &d1_staging.join("data.sqlite"),
            account,
            d1.id,
            1,
            64 * 1024 * 1024,
        )
        .unwrap(),
    );
    d1_paths
        .publish_staging(&d1_staging, account, d1.id)
        .unwrap();
    for resource in [kv.id, d1.id] {
        ResourceRepository::new(storage.db())
            .mark_ready(resource, 2)
            .unwrap();
    }

    let owned = crate::inspect_current_schema(storage.data_dir(), storage.db(), 5_000).unwrap();
    assert_eq!(owned.kv_files, 1);
    assert_eq!(owned.d1_files, 1);
    assert_eq!(owned.kv, crate::KV_SCHEMA_VERSION);
    assert_eq!(owned.d1, crate::D1_DATABASE_SCHEMA_VERSION);
    storage
        .bind_object_authority(ObjectStorageKind::Local, &[0xdd; 32])
        .unwrap();
    drop(storage);

    let data_dir = DataDir::acquire_existing_offline(&config).unwrap();
    let control =
        crate::ControlDb::open_readonly_wal_aware(&data_dir.control_db_path(), 5_000).unwrap();
    let offline = crate::inspect_current_schema(&data_dir, &control, 5_000).unwrap();
    assert_eq!(offline, owned);
    let key = crate::inspect_master_key(&config).unwrap();
    let snapshot_id = uuid::Uuid::now_v7().hyphenated().to_string();
    let authority = crate::inspect_control_db(&data_dir.control_db_path(), 5_000)
        .unwrap()
        .1;
    let object_prefix = format!(
        "system/snapshots/v1/{}/{snapshot_id}/objects/",
        authority.platform_id
    );
    let hardening = HardeningConfig::default();
    let request = crate::PreparePlatformSnapshotRequest {
        snapshot_id: &snapshot_id,
        label: "resource-schema-snapshot",
        created_at_ms: 2,
        release: p1_release_identity(),
        master_key_fingerprint: key.fingerprint(),
        object_backend_kind: ObjectStorageKind::Local,
        object_authority_fingerprint: &"d".repeat(64),
        r2_prefix_fingerprint: &"e".repeat(64),
        config_policy_sha256: &"f".repeat(64),
        object_prefix: &object_prefix,
        hardening: &hardening,
        sqlite_busy_timeout_ms: 5_000,
    };
    assert!(
        crate::estimate_platform_snapshot_bytes(
            &data_dir,
            &request,
            &authority.platform_id.to_string()
        )
        .unwrap()
            > 0
    );
    let prepared = crate::prepare_platform_snapshot(&data_dir, &request).unwrap();
    assert!(
        prepared
            .manifest
            .files
            .iter()
            .any(|file| file.role == open_compute_core::SnapshotFileRole::KvSqlite)
    );
    assert!(
        prepared
            .manifest
            .files
            .iter()
            .any(|file| file.role == open_compute_core::SnapshotFileRole::D1Sqlite)
    );
    assert_eq!(
        crate::inspect_current_schema(&data_dir, &control, 5_000).unwrap(),
        owned
    );
    let directory = kv_paths.namespace_dir(account, kv.id);
    let relocated = directory.with_extension("held");
    fs::rename(&directory, &relocated).unwrap();
    std::os::unix::fs::symlink(&relocated, &directory).unwrap();
    assert_eq!(
        crate::inspect_current_schema(&data_dir, &control, 5_000)
            .unwrap_err()
            .code(),
        ErrorCode::PathInvalid
    );
    fs::remove_file(&directory).unwrap();
    fs::rename(&relocated, &directory).unwrap();
    let file = kv_paths.database_path(account, kv.id);
    fs::remove_file(&file).unwrap();
    assert!(crate::inspect_current_schema(&data_dir, &control, 5_000).is_err());
    assert!(
        !file.exists(),
        "inspection must not recreate missing authority"
    );
}
