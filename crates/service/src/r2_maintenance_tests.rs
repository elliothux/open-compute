use super::*;
use open_compute_artifacts::{
    Fault, MapEnv, MockS3, S3ArtifactClient, resolve_s3_credentials_with,
};
use open_compute_core::config::StorageConfig;
use open_compute_core::{
    BindingKind, PlatformConfig, RequestId, ResourceAvailability, SystemClock,
};
use open_compute_storage::{ReserveResourceCreate, ResourceCreateReservation, ResourceRepository};
use std::time::Duration;

#[tokio::test]
async fn probes_debounce_provider_failures_isolate_collision_and_recover() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("data");
    let storage = Arc::new(
        PlatformStorage::bootstrap(
            &StorageConfig {
                data_dir: root.clone(),
                master_key_file: root.join("keys/master.key"),
                master_key_env: None,
                sqlite_busy_timeout_ms: 5_000,
                free_space_soft_bytes: 1_073_741_824,
                free_space_hard_bytes: 1,
            },
            &SystemClock,
        )
        .unwrap(),
    );
    let mock = MockS3::spawn("open-compute").await;
    let s3 = PlatformConfig::from_toml_str(&format!(
        r#"
[s3]
endpoint = "{}"
bucket = "open-compute"
prefix = "system/"
r2_prefix = "tenant/r2/"
connect_timeout_ms = 100
request_timeout_ms = 1000
"#,
        mock.endpoint
    ))
    .unwrap()
    .s3;
    let credentials = resolve_s3_credentials_with(
        &s3,
        &MapEnv::new()
            .with("S3_ACCESS_KEY_ID", "test-access")
            .with("S3_SECRET_ACCESS_KEY", "test-secret"),
    )
    .unwrap();
    let objects =
        R2ObjectStore::new(S3ArtifactClient::connect(&s3, &credentials, 1024 * 1024).unwrap());
    let config = R2Config {
        operation_timeout_ms: 500,
        ..R2Config::default()
    };
    let account = storage.identity().default_account_id;
    let resource_id = ResourceId::generate();
    let fingerprint = storage.crypto().fingerprint_request(b"r2-maintenance");
    let ResourceCreateReservation::Reserved(resource) = ResourceRepository::new(storage.db())
        .reserve_create(&ReserveResourceCreate {
            account_id: account,
            kind: BindingKind::R2Bucket,
            name: "maintenance",
            idempotency_key: "r2-maintenance",
            fingerprint_key_id: storage.crypto().fingerprint_key_id(),
            request_fingerprint: &fingerprint,
            resource_id,
            driver_schema_version: open_compute_storage::R2_SCHEMA_VERSION,
            request_id: RequestId::generate(),
            now_ms: 10,
            expires_at_ms: 10_000,
        })
        .unwrap()
    else {
        panic!("expected reserved R2 resource")
    };
    R2ResourceDriver::new(&storage, objects.clone(), config.clone())
        .create(&resource)
        .await
        .unwrap();
    ResourceRepository::new(storage.db())
        .mark_ready(resource_id, 11)
        .unwrap();

    let health = HealthCoordinator::new();
    health
        .set_component(
            ComponentName::S3,
            ComponentState::Healthy,
            Some(ReadinessReason::Ready),
        )
        .unwrap();
    let mut maintenance = R2Maintenance::default();
    maintenance.run(&storage, &objects, &config, &health).await;
    assert!(
        R2BucketRepository::new(storage.db())
            .get(account, resource_id)
            .unwrap()
            .last_probe_at_ms
            .is_some()
    );

    mock.set_fault(Fault::NotFound);
    maintenance.run(&storage, &objects, &config, &health).await;
    assert_eq!(
        ResourceRepository::new(storage.db())
            .get(account, resource_id)
            .unwrap()
            .availability,
        ResourceAvailability::Unavailable
    );

    mock.set_fault(Fault::None);
    maintenance.run(&storage, &objects, &config, &health).await;
    assert_eq!(
        ResourceRepository::new(storage.db())
            .get(account, resource_id)
            .unwrap()
            .availability,
        ResourceAvailability::Healthy
    );

    mock.set_fault(Fault::Auth);
    maintenance.run(&storage, &objects, &config, &health).await;
    assert_eq!(
        ResourceRepository::new(storage.db())
            .get(account, resource_id)
            .unwrap()
            .availability,
        ResourceAvailability::Healthy
    );
    maintenance.provider_failures.insert(
        resource_id,
        Instant::now() - MIN_PROVIDER_DEBOUNCE - Duration::from_secs(1),
    );
    maintenance.run(&storage, &objects, &config, &health).await;
    assert_eq!(
        ResourceRepository::new(storage.db())
            .get(account, resource_id)
            .unwrap()
            .availability,
        ResourceAvailability::Degraded
    );
    assert_eq!(
        health
            .snapshot()
            .components
            .into_iter()
            .find(|component| component.name == ComponentName::S3)
            .unwrap()
            .state,
        ComponentState::Degraded
    );
    maintenance.run(&storage, &objects, &config, &health).await;
    assert_eq!(
        ResourceRepository::new(storage.db())
            .get(account, resource_id)
            .unwrap()
            .availability,
        ResourceAvailability::Unavailable
    );

    mock.set_fault(Fault::None);
    maintenance.run(&storage, &objects, &config, &health).await;
    assert_eq!(
        ResourceRepository::new(storage.db())
            .get(account, resource_id)
            .unwrap()
            .availability,
        ResourceAvailability::Healthy
    );
    assert_eq!(
        health
            .snapshot()
            .components
            .into_iter()
            .find(|component| component.name == ComponentName::S3)
            .unwrap()
            .state,
        ComponentState::Healthy
    );
}
