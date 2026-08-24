use super::*;
use crate::{CreateResourceOutcome, CreateResourceRequest, ResourceController, ResourcePins};
use open_compute_core::config::StorageConfig;
use open_compute_core::{BindingKind, RequestId, SystemClock};
use open_compute_storage::{KvNamespaceRepository, PlatformStorage, ResourceRepository};
use std::time::Duration;

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

#[tokio::test]
async fn real_driver_creates_renames_reconciles_and_quarantines() {
    let (_temp, storage) = storage();
    let account = storage.identity().default_account_id;
    let pins = ResourcePins::new();
    let driver = KvResourceDriver::new(&storage, 256 * 1024 * 1024);
    let controller = ResourceController::new(&storage, pins.clone(), driver);
    let created = controller
        .create(&CreateResourceRequest {
            account_id: account,
            kind: BindingKind::KvNamespace,
            name: "cache".to_owned(),
            idempotency_key: "create-cache".to_owned(),
            driver_schema_version: 1,
            request_id: RequestId::generate(),
            now_ms: 10,
        })
        .unwrap();
    let resource_id = match created {
        CreateResourceOutcome::Applied(result) => result.resource_id,
        CreateResourceOutcome::Replay(_) => panic!("first create replayed"),
    };
    let record = KvNamespaceRepository::new(storage.db())
        .get(account, resource_id)
        .unwrap();
    assert_eq!(record.resource.name, "cache");
    assert!(
        storage
            .data_dir()
            .root()
            .join("kv")
            .join(account.to_string())
            .join(resource_id.to_string())
            .join("data.sqlite")
            .is_file()
    );

    controller
        .rename(account, resource_id, "renamed", RequestId::generate(), 20)
        .unwrap();
    assert_eq!(
        controller
            .reconcile_pending(RequestId::generate(), 21)
            .unwrap(),
        0
    );
    controller.refresh_health(account, resource_id, 22).unwrap();

    let pin = pins.try_pin(resource_id).unwrap();
    let blocked = controller
        .delete(
            account,
            resource_id,
            RequestId::generate(),
            30,
            Duration::from_millis(1),
        )
        .await
        .unwrap_err();
    assert_eq!(blocked.code(), ErrorCode::ResourceReferenced);
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
    assert!(
        KvNamespaceRepository::new(storage.db())
            .get(account, resource_id)
            .is_err()
    );
}

#[test]
fn driver_health_isolates_identity_corruption() {
    let (_temp, storage) = storage();
    let account = storage.identity().default_account_id;
    let pins = ResourcePins::new();
    let driver = KvResourceDriver::new(&storage, 256 * 1024 * 1024);
    let controller = ResourceController::new(&storage, pins.clone(), driver);
    let created = controller
        .create(&CreateResourceRequest {
            account_id: account,
            kind: BindingKind::KvNamespace,
            name: "corrupt".to_owned(),
            idempotency_key: "create-corrupt".to_owned(),
            driver_schema_version: 1,
            request_id: RequestId::generate(),
            now_ms: 10,
        })
        .unwrap();
    let id = match created {
        CreateResourceOutcome::Applied(value) => value.resource_id,
        _ => unreachable!(),
    };
    let record = KvNamespaceRepository::new(storage.db())
        .get(account, id)
        .unwrap();
    let path = KvPaths::open(storage.data_dir().root())
        .unwrap()
        .resolve_storage_key(&record.storage_key, account, id)
        .unwrap();
    let conn = rusqlite::Connection::open(path).unwrap();
    conn.execute(
        "UPDATE kv_meta SET value = ?1 WHERE key = 'resource_id'",
        [b"wrong".as_slice()],
    )
    .unwrap();
    drop(conn);
    let health = ResourceController::new(
        &storage,
        pins,
        KvResourceDriver::new(&storage, 256 * 1024 * 1024),
    )
    .refresh_health(account, id, 40)
    .unwrap();
    assert_eq!(health.availability, ResourceAvailability::Unavailable);
    assert_eq!(health.availability_code.as_deref(), Some("KV_CORRUPT"));
}

#[test]
fn driver_reconcile_delete_and_invalid_staging_matrix_is_fail_closed() {
    let (_temp, storage) = storage();
    let account = storage.identity().default_account_id;
    let pins = ResourcePins::new();
    let controller = ResourceController::new(
        &storage,
        pins,
        KvResourceDriver::new(&storage, 256 * 1024 * 1024),
    );
    let created = controller
        .create(&CreateResourceRequest {
            account_id: account,
            kind: BindingKind::KvNamespace,
            name: "matrix".to_owned(),
            idempotency_key: "create-matrix".to_owned(),
            driver_schema_version: 1,
            request_id: RequestId::generate(),
            now_ms: 10,
        })
        .unwrap();
    let id = match created {
        CreateResourceOutcome::Applied(value) => value.resource_id,
        _ => unreachable!(),
    };
    let ready = ResourceRepository::new(storage.db())
        .get(account, id)
        .unwrap();
    let driver = KvResourceDriver::new(&storage, 256 * 1024 * 1024);
    assert_eq!(driver.kind(), BindingKind::KvNamespace);
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

    let paths = KvPaths::open(storage.data_dir().root()).unwrap();
    assert_eq!(
        driver.reconcile(&creating).unwrap(),
        ReconcileOutcome::Absent
    );
    let corrupt = paths.create_namespace_staging(id).unwrap();
    std::fs::write(corrupt.join("data.sqlite"), b"not sqlite").unwrap();
    assert_eq!(
        driver.reconcile(&creating).unwrap(),
        ReconcileOutcome::Absent
    );
    assert!(!corrupt.exists());

    let first = paths.create_namespace_staging(id).unwrap();
    let second = paths.create_namespace_staging(id).unwrap();
    assert_eq!(
        driver.reconcile(&creating).unwrap_err().code(),
        ErrorCode::ResourceInvariantViolation
    );
    assert_eq!(
        driver.create(&creating).unwrap_err().code(),
        ErrorCode::ResourceInvariantViolation
    );
    paths.remove_namespace_staging(&first).unwrap();
    paths.remove_namespace_staging(&second).unwrap();

    let valid = paths.create_namespace_staging(id).unwrap();
    KvEngine::create(
        &valid.join("data.sqlite"),
        account,
        id,
        creating.created_at_ms,
        256 * 1024 * 1024,
    )
    .unwrap();
    assert_eq!(
        driver.reconcile(&creating).unwrap(),
        ReconcileOutcome::Ready
    );
    assert!(paths.database_path(account, id).is_file());

    driver.begin_delete(&deleting).unwrap();
    assert_eq!(
        driver.reconcile(&deleting).unwrap(),
        ReconcileOutcome::Deleted
    );
    let unavailable = driver.health(&ready).unwrap();
    assert_eq!(unavailable.availability, ResourceAvailability::Unavailable);
    assert_eq!(unavailable.code, Some("KV_UNAVAILABLE"));
    driver.finalize_delete(&deleting).unwrap();

    let mut tombstoned = ready.clone();
    tombstoned.state = ResourceState::Tombstoned;
    assert_eq!(
        driver.reconcile(&tombstoned).unwrap(),
        ReconcileOutcome::Deleted
    );
    let mut absent = creating.clone();
    absent.id = open_compute_core::ResourceId::generate();
    assert_eq!(driver.reconcile(&absent).unwrap(), ReconcileOutcome::Absent);
    assert_eq!(
        driver.health(&absent).unwrap_err().code(),
        ErrorCode::ResourceNotFound
    );
    assert_eq!(
        KvResourceDriver::new(&storage, 1)
            .create(&creating)
            .unwrap_err()
            .code(),
        ErrorCode::ResourceInvariantViolation
    );
}
