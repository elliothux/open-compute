use super::*;
use open_compute_core::SystemClock;
use open_compute_core::config::StorageConfig;
use open_compute_storage::PlatformStorage;
use std::collections::HashSet;
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug, Default)]
struct FakeDriver {
    ready: Arc<Mutex<HashSet<ResourceId>>>,
    deleted: Arc<Mutex<HashSet<ResourceId>>>,
    unavailable: Arc<Mutex<bool>>,
    fail_delete: Arc<Mutex<bool>>,
}

impl ResourceDriver for FakeDriver {
    fn kind(&self) -> BindingKind {
        BindingKind::KvNamespace
    }

    fn create(&self, resource: &ResourceRecord) -> Result<(), PlatformError> {
        self.ready.lock().unwrap().insert(resource.id);
        Ok(())
    }

    fn reconcile(&self, resource: &ResourceRecord) -> Result<ReconcileOutcome, PlatformError> {
        if self.deleted.lock().unwrap().contains(&resource.id) {
            Ok(ReconcileOutcome::Deleted)
        } else if self.ready.lock().unwrap().contains(&resource.id) {
            Ok(ReconcileOutcome::Ready)
        } else {
            Ok(ReconcileOutcome::Absent)
        }
    }

    fn begin_delete(&self, resource: &ResourceRecord) -> Result<(), PlatformError> {
        self.ready.lock().unwrap().remove(&resource.id);
        self.deleted.lock().unwrap().insert(resource.id);
        Ok(())
    }

    fn finalize_delete(&self, resource: &ResourceRecord) -> Result<(), PlatformError> {
        if *self.fail_delete.lock().unwrap() {
            return Err(PlatformError::new(
                ErrorCode::ResourceUnavailable,
                "fake delete failed",
            ));
        }
        if !self.deleted.lock().unwrap().contains(&resource.id) {
            return Err(PlatformError::new(
                ErrorCode::ResourceInvariantViolation,
                "fake delete was not begun",
            ));
        }
        Ok(())
    }

    fn health(&self, _resource: &ResourceRecord) -> Result<ResourceHealth, PlatformError> {
        if *self.unavailable.lock().unwrap() {
            Ok(ResourceHealth {
                availability: ResourceAvailability::Unavailable,
                code: Some("FAKE_UNAVAILABLE"),
            })
        } else {
            Ok(ResourceHealth::healthy())
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct AlternateDriver;

impl ResourceDriver for AlternateDriver {
    fn kind(&self) -> BindingKind {
        BindingKind::R2Bucket
    }

    fn create(&self, _resource: &ResourceRecord) -> Result<(), PlatformError> {
        Ok(())
    }

    fn reconcile(&self, _resource: &ResourceRecord) -> Result<ReconcileOutcome, PlatformError> {
        Ok(ReconcileOutcome::Ready)
    }

    fn begin_delete(&self, _resource: &ResourceRecord) -> Result<(), PlatformError> {
        Ok(())
    }

    fn finalize_delete(&self, _resource: &ResourceRecord) -> Result<(), PlatformError> {
        Ok(())
    }

    fn health(&self, _resource: &ResourceRecord) -> Result<ResourceHealth, PlatformError> {
        Ok(ResourceHealth::healthy())
    }
}

#[derive(Clone, Copy, Debug)]
struct StuckDriver(ReconcileOutcome);

impl ResourceDriver for StuckDriver {
    fn kind(&self) -> BindingKind {
        BindingKind::KvNamespace
    }

    fn create(&self, _resource: &ResourceRecord) -> Result<(), PlatformError> {
        Ok(())
    }

    fn reconcile(&self, _resource: &ResourceRecord) -> Result<ReconcileOutcome, PlatformError> {
        Ok(self.0)
    }

    fn begin_delete(&self, _resource: &ResourceRecord) -> Result<(), PlatformError> {
        Ok(())
    }

    fn finalize_delete(&self, _resource: &ResourceRecord) -> Result<(), PlatformError> {
        Ok(())
    }

    fn health(&self, _resource: &ResourceRecord) -> Result<ResourceHealth, PlatformError> {
        Ok(ResourceHealth::healthy())
    }
}

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

fn request(account_id: AccountId, key: &str, now_ms: i64) -> CreateResourceRequest {
    CreateResourceRequest {
        account_id,
        kind: BindingKind::KvNamespace,
        name: "cache".to_owned(),
        idempotency_key: key.to_owned(),
        driver_schema_version: 1,
        request_id: RequestId::generate(),
        now_ms,
    }
}

#[tokio::test]
async fn create_replay_health_and_delete_wait_for_pin() {
    let (_temp, storage) = storage();
    let account = storage.identity().default_account_id;
    let driver = FakeDriver::default();
    let pins = ResourcePins::new();
    let controller = ResourceController::new(&storage, pins.clone(), driver.clone());
    let created = match controller.create(&request(account, "create", 10)).unwrap() {
        CreateResourceOutcome::Applied(result) => result,
        other => panic!("unexpected {other:?}"),
    };
    assert_eq!(created.state, ResourceState::Ready);
    assert_eq!(
        controller.get(account, created.resource_id).unwrap().name,
        "cache"
    );
    assert_eq!(controller.list(account).unwrap().len(), 1);
    assert_eq!(
        controller
            .rename(
                account,
                created.resource_id,
                "renamed",
                RequestId::generate(),
                11,
            )
            .unwrap()
            .name,
        "renamed"
    );
    assert!(matches!(
        controller.create(&request(account, "create", 11)).unwrap(),
        CreateResourceOutcome::Replay(bytes) if !bytes.is_empty()
    ));
    *driver.unavailable.lock().unwrap() = true;
    assert_eq!(
        controller
            .refresh_health(account, created.resource_id, 12)
            .unwrap()
            .availability,
        ResourceAvailability::Unavailable
    );
    *driver.unavailable.lock().unwrap() = false;
    assert_eq!(
        controller
            .refresh_health(account, created.resource_id, 13)
            .unwrap()
            .availability,
        ResourceAvailability::Healthy
    );

    let pin = pins.try_pin(created.resource_id).unwrap();
    let deleting = controller.delete(
        account,
        created.resource_id,
        RequestId::generate(),
        14,
        Duration::from_secs(1),
    );
    tokio::pin!(deleting);
    assert!(
        tokio::time::timeout(Duration::from_millis(5), &mut deleting)
            .await
            .is_err()
    );
    drop(pin);
    deleting.await.unwrap();
    assert_eq!(pins.count(created.resource_id), 0);
}

#[test]
fn startup_reconcile_converges_create_and_delete() {
    let (_temp, storage) = storage();
    let account = storage.identity().default_account_id;
    let driver = FakeDriver::default();
    let repo = ResourceRepository::new(storage.db());
    let fingerprint = [7; 32];
    let resource_id = ResourceId::generate();
    let resource = match repo
        .reserve_create(&ReserveResourceCreate {
            account_id: account,
            kind: BindingKind::KvNamespace,
            name: "cache",
            idempotency_key: "crash-create",
            fingerprint_key_id: "key",
            request_fingerprint: &fingerprint,
            resource_id,
            driver_schema_version: 1,
            request_id: RequestId::generate(),
            now_ms: 10,
            expires_at_ms: 100,
        })
        .unwrap()
    {
        ResourceCreateReservation::Reserved(resource) => resource,
        other => panic!("unexpected {other:?}"),
    };
    driver.create(&resource).unwrap();
    let controller = ResourceController::new(&storage, ResourcePins::new(), driver.clone());
    assert_eq!(
        controller
            .reconcile_pending(RequestId::generate(), 11)
            .unwrap(),
        1
    );
    repo.begin_delete(account, resource_id, 12).unwrap();
    driver
        .begin_delete(&repo.get(account, resource_id).unwrap())
        .unwrap();
    assert_eq!(
        controller
            .reconcile_pending(RequestId::generate(), 13)
            .unwrap(),
        1
    );
    assert_eq!(
        repo.get(account, resource_id).unwrap().state,
        ResourceState::Tombstoned
    );
}

#[tokio::test]
async fn lifecycle_rejects_wrong_driver_invalid_state_and_unfences_failed_delete() {
    let (_temp, storage) = storage();
    let account = storage.identity().default_account_id;
    let driver = FakeDriver::default();
    let pins = ResourcePins::new();
    let controller = ResourceController::new(&storage, pins.clone(), driver.clone());

    let mut wrong_kind = request(account, "wrong-kind", 10);
    wrong_kind.kind = BindingKind::R2Bucket;
    assert_eq!(
        controller.create(&wrong_kind).unwrap_err().code(),
        ErrorCode::BindingTypeMismatch
    );
    let mut invalid_schema = request(account, "invalid-schema", 10);
    invalid_schema.driver_schema_version = 0;
    assert_eq!(
        controller.create(&invalid_schema).unwrap_err().code(),
        ErrorCode::BindingTypeMismatch
    );

    let created = match controller
        .create(&request(account, "delete-fails", 11))
        .unwrap()
    {
        CreateResourceOutcome::Applied(result) => result,
        other => panic!("unexpected {other:?}"),
    };
    let alternate = ResourceController::new(&storage, pins.clone(), AlternateDriver);
    assert_eq!(
        alternate
            .get(account, created.resource_id)
            .unwrap_err()
            .code(),
        ErrorCode::ResourceNotFound
    );
    assert_eq!(
        alternate
            .refresh_health(account, created.resource_id, 12)
            .unwrap_err()
            .code(),
        ErrorCode::ResourceNotReady
    );
    assert_eq!(
        alternate
            .delete(
                account,
                created.resource_id,
                RequestId::generate(),
                12,
                Duration::from_millis(10),
            )
            .await
            .unwrap_err()
            .code(),
        ErrorCode::ResourceNotFound
    );

    *driver.fail_delete.lock().unwrap() = true;
    assert_eq!(
        controller
            .delete(
                account,
                created.resource_id,
                RequestId::generate(),
                13,
                Duration::from_millis(10),
            )
            .await
            .unwrap_err()
            .code(),
        ErrorCode::ResourceUnavailable
    );
    assert!(pins.try_pin(created.resource_id).is_ok());
}

#[test]
fn create_reconciliation_fails_closed_on_impossible_driver_outcomes() {
    for (key, outcome) in [
        ("reports-deleted", ReconcileOutcome::Deleted),
        ("stays-absent", ReconcileOutcome::Absent),
    ] {
        let (_temp, storage) = storage();
        let account = storage.identity().default_account_id;
        let controller =
            ResourceController::new(&storage, ResourcePins::new(), StuckDriver(outcome));
        assert_eq!(
            controller
                .create(&request(account, key, 10))
                .unwrap_err()
                .code(),
            ErrorCode::ResourceInvariantViolation
        );
    }
}

#[test]
fn health_rejects_a_resource_that_is_still_creating() {
    let (_temp, storage) = storage();
    let account = storage.identity().default_account_id;
    let resource_id = ResourceId::generate();
    let fingerprint = [9; 32];
    ResourceRepository::new(storage.db())
        .reserve_create(&ReserveResourceCreate {
            account_id: account,
            kind: BindingKind::KvNamespace,
            name: "creating",
            idempotency_key: "creating",
            fingerprint_key_id: "key",
            request_fingerprint: &fingerprint,
            resource_id,
            driver_schema_version: 1,
            request_id: RequestId::generate(),
            now_ms: 10,
            expires_at_ms: 100,
        })
        .unwrap();
    let controller = ResourceController::new(&storage, ResourcePins::new(), FakeDriver::default());
    assert_eq!(
        controller
            .refresh_health(account, resource_id, 11)
            .unwrap_err()
            .code(),
        ErrorCode::ResourceNotReady
    );
}

#[test]
fn create_reconciliation_accepts_ready_and_rejects_tombstoned_rows() {
    let (_temp, storage) = storage();
    let account = storage.identity().default_account_id;
    let controller = ResourceController::new(&storage, ResourcePins::new(), FakeDriver::default());
    let mut resource = ResourceRecord {
        id: ResourceId::generate(),
        account_id: account,
        kind: BindingKind::KvNamespace,
        name: "lifecycle".to_owned(),
        state: ResourceState::Ready,
        availability: ResourceAvailability::Healthy,
        availability_code: None,
        spec_generation: 1,
        driver_schema_version: 1,
        created_at_ms: 10,
        updated_at_ms: 10,
        deleted_at_ms: None,
    };
    assert_eq!(
        controller.reconcile_create(resource.clone(), 11).unwrap(),
        resource
    );
    resource.state = ResourceState::Tombstoned;
    assert_eq!(
        controller
            .reconcile_create(resource, 11)
            .unwrap_err()
            .code(),
        ErrorCode::ResourceNotReady
    );
}
