use super::*;

#[test]
fn p1_schema_inspection_checks_current_kv_and_d1_files_without_mutation() {
    let (_tmp, root) = unique_root();
    let config = storage_config(&root);
    let storage = PlatformStorage::bootstrap(&config, &SystemClock).unwrap();
    let scheduler_path = storage.data_dir().ensure_scheduler_db().unwrap();
    drop(crate::SchedulerStore::open(&scheduler_path, 5_000, 1).unwrap());
    let account = storage.identity().default_account_id;

    let reserve = |kind, name: &str, key: &str, driver_schema_version: i64| {
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
                    driver_schema_version: driver_schema_version.try_into().unwrap(),
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

    let kv = reserve(BindingKind::KvNamespace, "schema-kv", "schema-kv", 1);
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

    let d1 = reserve(BindingKind::D1Database, "schema-d1", "schema-d1", 1);
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

    // A Vectorize index and an AI Search instance with pre-Refinery legacy database heads
    // exercise their inspection arms and legacy adoption closures.
    let vectorize = reserve(
        BindingKind::VectorizeIndex,
        "schema-vectorize",
        "schema-vectorize",
        i64::from(crate::vectorize::VECTORIZE_SCHEMA_VERSION),
    );
    let vectorize_key = crate::VectorizePaths::storage_key(account, vectorize.id);
    crate::vectorize::VectorizeIndexRepository::new(storage.db())
        .ensure_index(
            &vectorize,
            &vectorize_key,
            crate::vectorize::VECTORIZE_SCHEMA_VERSION,
            32,
            "cosine",
            100,
            16 * 1024 * 1024,
        )
        .unwrap();
    let vectorize_root = root
        .join("vectorize")
        .join(account.to_string())
        .join(vectorize.id.to_string());
    fs::create_dir_all(&vectorize_root).unwrap();
    use std::os::unix::fs::PermissionsExt as _;
    fs::write(vectorize_root.join("data.sqlite"), b"").unwrap();
    fs::set_permissions(
        vectorize_root.join("data.sqlite"),
        fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    let vectorize_connection = Connection::open(vectorize_root.join("data.sqlite")).unwrap();
    vectorize_connection
        .execute_batch(include_str!(
            "../../refinery-migrations/vectorize/V1__init.sql"
        ))
        .unwrap();
    vectorize_connection
        .execute_batch(
            "ALTER TABLE index_meta ADD COLUMN schema_version INTEGER NOT NULL DEFAULT 0;",
        )
        .unwrap();
    vectorize_connection
        .execute(
            "INSERT INTO index_meta(singleton, resource_id, dimensions, metric, quota_vectors,
               quota_bytes, schema_version) VALUES(1, ?1, 32, 'cosine', 100, 16777216, ?2)",
            rusqlite::params![
                vectorize.id.to_string(),
                i64::from(crate::vectorize::VECTORIZE_SCHEMA_VERSION)
            ],
        )
        .unwrap();
    drop(vectorize_connection);

    let ai_namespace = reserve(
        BindingKind::AiSearchNamespace,
        "schema-ai-search-namespace",
        "schema-ai-search-namespace",
        i64::from(crate::ai_search::AI_SEARCH_NAMESPACE_SCHEMA_VERSION),
    );
    let catalog = crate::ai_search::AiSearchCatalog::new(storage.db());
    catalog.ensure_namespace(&ai_namespace).unwrap();
    ResourceRepository::new(storage.db())
        .mark_ready(ai_namespace.id, 3)
        .unwrap();
    let ai_search = reserve(
        BindingKind::AiSearchInstance,
        "schema-ai-search",
        "schema-ai-search",
        i64::from(crate::ai_search::AI_SEARCH_SCHEMA_VERSION),
    );
    let ai_search_key = crate::AiSearchPaths::storage_key(account, ai_search.id);
    catalog
        .ensure_instance(
            &ai_search,
            ai_namespace.id,
            "primary_v1",
            &ai_search_key,
            crate::ai_search::AI_SEARCH_SCHEMA_VERSION,
            [7; 32],
        )
        .unwrap();
    let ai_search_root = root
        .join("ai-search")
        .join(account.to_string())
        .join(ai_search.id.to_string());
    fs::create_dir_all(&ai_search_root).unwrap();
    fs::write(ai_search_root.join("data.sqlite"), b"").unwrap();
    fs::set_permissions(
        ai_search_root.join("data.sqlite"),
        fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    let ai_search_connection = Connection::open(ai_search_root.join("data.sqlite")).unwrap();
    ai_search_connection
        .execute_batch(include_str!(
            "../../refinery-migrations/ai_search/V1__init.sql"
        ))
        .unwrap();
    ai_search_connection
        .execute_batch(
            "ALTER TABLE instance_meta ADD COLUMN schema_version INTEGER NOT NULL DEFAULT 0;",
        )
        .unwrap();
    ai_search_connection
        .execute(
            "INSERT INTO instance_meta(singleton, resource_id, model_contract_sha256,
               model_contract_json, public_config_json, dimensions, vector_enabled,
               keyword_enabled, active_index_generation, active_epoch, config_generation,
               created_at_ms, updated_at_ms, schema_version)
             VALUES(1, ?1, ?2, X'7B7D', X'7B7D', 0, 0, 1, 1, 1, 1, 0, 0, ?3)",
            rusqlite::params![
                ai_search.id.to_string(),
                [7u8; 32].as_slice(),
                i64::from(crate::ai_search::AI_SEARCH_SCHEMA_VERSION)
            ],
        )
        .unwrap();
    drop(ai_search_connection);
    for resource in [vectorize.id, ai_search.id] {
        ResourceRepository::new(storage.db())
            .mark_ready(resource, 3)
            .unwrap();
    }

    let owned = crate::inspect_current_schema(storage.data_dir(), storage.db(), 5_000).unwrap();
    assert_eq!(owned.kv_files, 1);
    assert_eq!(owned.d1_files, 1);
    assert_eq!(owned.vectorize_files, 1);
    assert_eq!(owned.ai_search_files, 1);
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
