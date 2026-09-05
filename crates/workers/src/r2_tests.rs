use super::*;
use open_compute_artifacts::{
    MapEnv, ObjectBackend, R2PutOptions, R2UploadSource, UserObjectKey, hash_bytes,
};
use open_compute_core::config::DataConfig;
use open_compute_core::{RequestId, SystemClock};
use open_compute_storage::{ReserveResourceCreate, ResourceCreateReservation, ResourceRepository};
use std::os::unix::fs::PermissionsExt as _;

fn storage_fixture() -> (tempfile::TempDir, PlatformStorage, ResourceRecord) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("data");
    let storage = PlatformStorage::bootstrap(
        &DataConfig {
            path: root.clone(),
            master_key_file: root.join("keys/master.key"),
            master_key_env: None,
            sqlite_busy_timeout_ms: 5_000,
            free_space_soft_bytes: 1_073_741_824,
            free_space_hard_bytes: 268_435_456,
        },
        &SystemClock,
    )
    .unwrap();
    let fingerprint = storage.crypto().fingerprint_request(b"r2-driver");
    let reservation = ResourceRepository::new(storage.db())
        .reserve_create(
            &ReserveResourceCreate {
                account_id: storage.identity().default_account_id,
                kind: BindingKind::R2Bucket,
                name: "images",
                idempotency_key: "r2-driver",
                fingerprint_key_id: storage.crypto().fingerprint_key_id(),
                request_fingerprint: &fingerprint,
                resource_id: open_compute_core::ResourceId::generate(),
                driver_schema_version: R2_SCHEMA_VERSION,
                request_id: RequestId::generate(),
                now_ms: 10,
                expires_at_ms: 1_000,
            },
            1_000_000,
        )
        .unwrap();
    let ResourceCreateReservation::Reserved(resource) = reservation else {
        unreachable!()
    };
    (temp, storage, resource)
}

fn object_store(mock: &open_compute_artifacts::MockS3) -> R2ObjectStore {
    object_store_with_prefix(mock, "tenant/r2/")
}

fn object_store_with_prefix(
    mock: &open_compute_artifacts::MockS3,
    r2_prefix: &str,
) -> R2ObjectStore {
    let config = open_compute_core::S3Config {
        endpoint: mock.endpoint.clone(),
        bucket: "bucket".to_owned(),
        r2_prefix: r2_prefix.to_owned(),
        ..open_compute_core::S3Config::default()
    };
    let env = MapEnv::new()
        .with("S3_ACCESS_KEY_ID", "test-access")
        .with("S3_SECRET_ACCESS_KEY", "test-secret");
    let credentials = open_compute_artifacts::resolve_s3_credentials_with(&config, &env).unwrap();
    R2ObjectStore::new(ObjectBackend::connect_s3(&config, &credentials, 1024 * 1024).unwrap())
}

#[tokio::test]
async fn driver_creates_reconciles_refuses_nonempty_and_recovers_force_delete() {
    let mock = open_compute_artifacts::MockS3::spawn("bucket").await;
    let (_temp, storage, resource) = storage_fixture();
    let objects = object_store(&mock);
    let driver = R2ResourceDriver::new(&storage, objects.clone(), R2Config::default());
    let bucket = driver.create(&resource).await.unwrap();
    assert_eq!(driver.reconcile(&resource).await.unwrap(), bucket);
    ResourceRepository::new(storage.db())
        .mark_ready(resource.id, 11)
        .unwrap();
    let ready = R2BucketRepository::new(storage.db())
        .get(resource.account_id, resource.id)
        .unwrap();
    assert_eq!(
        driver
            .reconcile(&ready.resource)
            .await
            .unwrap()
            .physical_prefix,
        bucket.physical_prefix
    );
    let drifted = R2ResourceDriver::new(
        &storage,
        object_store_with_prefix(&mock, "tenant/other/"),
        R2Config::default(),
    );
    assert_eq!(
        drifted.reconcile(&ready.resource).await.unwrap_err().code(),
        ErrorCode::ResourceInvariantViolation
    );

    let staging = storage.data_dir().root().join("r2-test-upload");
    std::fs::write(&staging, b"value").unwrap();
    std::fs::set_permissions(&staging, std::fs::Permissions::from_mode(0o600)).unwrap();
    let source = R2UploadSource {
        path: staging,
        length: 5,
        checksums: hash_bytes(b"value"),
        version: uuid::Uuid::now_v7().hyphenated().to_string(),
    };
    let locator = driver.locator(&ready).unwrap();
    let key = UserObjectKey::parse("same/key").unwrap();
    objects
        .put_file(&locator, &key, &source, &R2PutOptions::default(), None)
        .await
        .unwrap();
    assert_eq!(
        driver.require_empty(&ready).await.unwrap_err().code(),
        ErrorCode::R2BucketNotEmpty
    );
    assert_eq!(driver.drain_objects(&ready).await.unwrap(), 1);
    driver.require_empty(&ready).await.unwrap();
    driver.finalize_delete(&ready).await.unwrap();
}
