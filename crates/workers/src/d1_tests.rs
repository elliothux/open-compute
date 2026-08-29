use super::*;
use crate::{CreateResourceOutcome, CreateResourceRequest, ResourceController, ResourcePins};
use open_compute_core::config::StorageConfig;
use open_compute_core::{RequestId, ResourceId, SystemClock};
use open_compute_storage::{
    D1DatabaseRepository, ReserveResourceCreate, ResourceCreateReservation, ResourceRepository,
};
use std::time::Duration;

const QUOTA: u64 = 256 * 1024 * 1024;

fn storage() -> (tempfile::TempDir, PlatformStorage) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("data");
    let config = StorageConfig {
        data_dir: root.clone(),
        master_key_file: root.join("keys/master.key"),
        master_key_env: None,
        sqlite_busy_timeout_ms: 5_000,
        free_space_soft_bytes: 1_073_741_824,
        free_space_hard_bytes: 268_435_456,
    };
    let storage = PlatformStorage::bootstrap(&config, &SystemClock).unwrap();
    (temp, storage)
}

fn create(storage: &PlatformStorage, name: &str) -> ResourceId {
    let account = storage.identity().default_account_id;
    let controller = ResourceController::new(
        storage,
        ResourcePins::new(),
        D1ResourceDriver::new(storage, QUOTA),
    );
    match controller
        .create(&CreateResourceRequest {
            account_id: account,
            kind: BindingKind::D1Database,
            name: name.to_owned(),
            idempotency_key: format!("create-{name}"),
            driver_schema_version: D1_DATABASE_SCHEMA_VERSION,
            request_id: RequestId::generate(),
            now_ms: 10,
        })
        .unwrap()
    {
        CreateResourceOutcome::Applied(result) => result.resource_id,
        CreateResourceOutcome::Replay(_) => panic!("first create replayed"),
    }
}

#[tokio::test]
async fn real_driver_creates_reconciles_fences_and_deletes() {
    let (_temp, storage) = storage();
    let account = storage.identity().default_account_id;
    let pins = ResourcePins::new();
    let controller = ResourceController::new(
        &storage,
        pins.clone(),
        D1ResourceDriver::new(&storage, QUOTA),
    );
    let resource_id = match controller
        .create(&CreateResourceRequest {
            account_id: account,
            kind: BindingKind::D1Database,
            name: "primary".to_owned(),
            idempotency_key: "create-primary".to_owned(),
            driver_schema_version: D1_DATABASE_SCHEMA_VERSION,
            request_id: RequestId::generate(),
            now_ms: 10,
        })
        .unwrap()
    {
        CreateResourceOutcome::Applied(result) => result.resource_id,
        CreateResourceOutcome::Replay(_) => panic!("first create replayed"),
    };
    let record = D1DatabaseRepository::new(storage.db())
        .get(account, resource_id)
        .unwrap();
    let path = D1Paths::open(storage.data_dir().root())
        .unwrap()
        .resolve_storage_key(&record.storage_key, account, resource_id)
        .unwrap();
    assert!(path.is_file());
    assert_eq!(
        controller
            .reconcile_pending(RequestId::generate(), 20)
            .unwrap(),
        0
    );
    assert_eq!(
        controller
            .refresh_health(account, resource_id, 21)
            .unwrap()
            .availability,
        ResourceAvailability::Healthy
    );

    let pin = pins.try_pin(resource_id).unwrap();
    assert_eq!(
        controller
            .delete(
                account,
                resource_id,
                RequestId::generate(),
                30,
                Duration::from_millis(1),
            )
            .await
            .unwrap_err()
            .code(),
        ErrorCode::ResourceReferenced
    );
    drop(pin);
    controller
        .delete(
            account,
            resource_id,
            RequestId::generate(),
            31,
            Duration::from_secs(1),
        )
        .await
        .unwrap();
    assert_eq!(
        ResourceRepository::new(storage.db())
            .get(account, resource_id)
            .unwrap()
            .state,
        ResourceState::Tombstoned
    );
    assert!(!path.exists());
}

#[test]
fn identity_damage_is_persisted_locally_without_affecting_another_database() {
    let (_temp, storage) = storage();
    let account = storage.identity().default_account_id;
    let damaged = create(&storage, "damaged");
    let healthy = create(&storage, "healthy");
    let record = D1DatabaseRepository::new(storage.db())
        .get(account, damaged)
        .unwrap();
    let paths = D1Paths::open(storage.data_dir().root()).unwrap();
    let path = paths
        .resolve_storage_key(&record.storage_key, account, damaged)
        .unwrap();
    let connection = rusqlite::Connection::open(path).unwrap();
    connection
        .execute(
            "UPDATE __open_compute_meta SET value = ?1 WHERE key = 'resource_id'",
            [b"wrong".as_slice()],
        )
        .unwrap();
    drop(connection);

    let controller = ResourceController::new(
        &storage,
        ResourcePins::new(),
        D1ResourceDriver::new(&storage, QUOTA),
    );
    let health = controller.refresh_health(account, damaged, 40).unwrap();
    assert_eq!(health.availability, ResourceAvailability::Unavailable);
    assert_eq!(
        health.availability_code.as_deref(),
        Some("D1_IDENTITY_MISMATCH")
    );
    let healthy_state = controller.refresh_health(account, healthy, 41).unwrap();
    assert_eq!(healthy_state.availability, ResourceAvailability::Healthy);
}

#[test]
fn reconcile_matrix_fails_closed_and_cleans_invalid_staging() {
    let (_temp, storage) = storage();
    let account = storage.identity().default_account_id;
    let id = create(&storage, "matrix");
    let ready = ResourceRepository::new(storage.db())
        .get(account, id)
        .unwrap();
    let driver = D1ResourceDriver::new(&storage, QUOTA);
    assert_eq!(driver.kind(), BindingKind::D1Database);
    assert_eq!(driver.reconcile(&ready).unwrap(), ReconcileOutcome::Ready);

    let mut creating = ready.clone();
    creating.state = ResourceState::Creating;
    let mut deleting = ready.clone();
    deleting.state = ResourceState::Deleting;
    assert_eq!(
        driver.reconcile(&deleting).unwrap(),
        ReconcileOutcome::Ready
    );
    assert!(driver.finalize_delete(&deleting).is_err());
    driver.begin_delete(&deleting).unwrap();
    assert_eq!(
        driver.reconcile(&deleting).unwrap(),
        ReconcileOutcome::Deleted
    );

    let paths = D1Paths::open(storage.data_dir().root()).unwrap();
    let invalid = paths.create_database_staging(id).unwrap();
    std::fs::write(invalid.join("data.sqlite"), b"not sqlite").unwrap();
    assert_eq!(
        driver.reconcile(&creating).unwrap(),
        ReconcileOutcome::Absent
    );
    assert!(!invalid.exists());

    let first = paths.create_database_staging(id).unwrap();
    let second = paths.create_database_staging(id).unwrap();
    assert_eq!(
        driver.reconcile(&creating).unwrap_err().code(),
        ErrorCode::ResourceInvariantViolation
    );
    paths.remove_operation_dir(&first).unwrap();
    paths.remove_operation_dir(&second).unwrap();

    let mut tombstoned = ready.clone();
    tombstoned.state = ResourceState::Tombstoned;
    assert_eq!(
        driver.reconcile(&tombstoned).unwrap(),
        ReconcileOutcome::Deleted
    );
    driver.finalize_delete(&deleting).unwrap();
    assert_eq!(
        D1ResourceDriver::new(&storage, 1)
            .create(&creating)
            .unwrap_err()
            .code(),
        ErrorCode::ResourceInvariantViolation
    );
}

#[test]
fn creating_database_keeps_its_frozen_quota_across_config_change() {
    let (_temp, storage) = storage();
    let account = storage.identity().default_account_id;
    let fingerprint = storage.crypto().fingerprint_request(b"frozen-d1-create");
    let ResourceCreateReservation::Reserved(resource) = ResourceRepository::new(storage.db())
        .reserve_create(
            &ReserveResourceCreate {
                account_id: account,
                kind: BindingKind::D1Database,
                name: "frozen-quota",
                idempotency_key: "frozen-quota",
                fingerprint_key_id: storage.crypto().fingerprint_key_id(),
                request_fingerprint: &fingerprint,
                resource_id: ResourceId::generate(),
                driver_schema_version: D1_DATABASE_SCHEMA_VERSION,
                request_id: RequestId::generate(),
                now_ms: 10,
                expires_at_ms: 1_000,
            },
            1_000_000,
        )
        .unwrap()
    else {
        panic!("first reservation must create the resource");
    };
    let storage_key = D1Paths::storage_key(account, resource.id);
    D1DatabaseRepository::new(storage.db())
        .ensure_database(&resource, &storage_key, D1_DATABASE_SCHEMA_VERSION, QUOTA)
        .unwrap();

    D1ResourceDriver::new(&storage, QUOTA * 2)
        .create(&resource)
        .unwrap();
    assert_eq!(
        D1DatabaseRepository::new(storage.db())
            .get(account, resource.id)
            .unwrap()
            .quota_bytes,
        QUOTA
    );
}

#[test]
fn restore_intent_is_deferred_and_cannot_fall_back_to_empty_create() {
    let (_temp, storage) = storage();
    let account = storage.identity().default_account_id;
    let fingerprint = storage.crypto().fingerprint_request(b"restore-intent");
    let ResourceCreateReservation::Reserved(resource) = ResourceRepository::new(storage.db())
        .reserve_create(
            &ReserveResourceCreate {
                account_id: account,
                kind: BindingKind::D1Database,
                name: "restore-intent",
                idempotency_key: "restore-intent",
                fingerprint_key_id: storage.crypto().fingerprint_key_id(),
                request_fingerprint: &fingerprint,
                resource_id: ResourceId::generate(),
                driver_schema_version: D1_DATABASE_SCHEMA_VERSION,
                request_id: RequestId::generate(),
                now_ms: 10,
                expires_at_ms: 1_000,
            },
            1_000_000,
        )
        .unwrap()
    else {
        panic!("first reservation must create the resource");
    };
    let backup_id = uuid::Uuid::now_v7().hyphenated().to_string();
    D1DatabaseRepository::new(storage.db())
        .create_backup(resource.id, &backup_id, 1, 0, "backup", &[0; 32], 10)
        .unwrap();
    let storage_key = D1Paths::storage_key(account, resource.id);
    D1DatabaseRepository::new(storage.db())
        .ensure_restoring_database(
            &resource,
            &storage_key,
            D1_DATABASE_SCHEMA_VERSION,
            QUOTA,
            &backup_id,
        )
        .unwrap();
    let driver = D1ResourceDriver::new(&storage, QUOTA);
    assert_eq!(
        driver.reconcile(&resource).unwrap(),
        ReconcileOutcome::Deferred
    );
    assert_eq!(
        driver.create(&resource).unwrap_err().code(),
        ErrorCode::ResourceNotReady
    );

    let mut unknown = resource;
    unknown.id = ResourceId::generate();
    assert_eq!(
        driver.health(&unknown).unwrap_err().code(),
        ErrorCode::ResourceNotFound
    );
}
