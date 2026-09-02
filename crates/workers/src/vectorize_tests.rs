use super::*;
use crate::{CreateResourceOutcome, CreateResourceRequest, ResourceController, ResourcePins};
use open_compute_core::config::StorageConfig;
use open_compute_core::{ErrorCode, RequestId, ResourceId, SystemClock};
use open_compute_storage::ResourceRepository;
use std::time::Duration;

fn storage() -> (tempfile::TempDir, PlatformStorage) {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join("data");
    let storage = PlatformStorage::bootstrap(
        &StorageConfig {
            data_dir: root.clone(),
            master_key_file: root.join("keys/master.key"),
            master_key_env: None,
            sqlite_busy_timeout_ms: 5_000,
            free_space_soft_bytes: 1_073_741_824,
            free_space_hard_bytes: 268_435_456,
        },
        &SystemClock,
    )
    .unwrap();
    (temporary, storage)
}

fn record(account_id: open_compute_core::AccountId, state: ResourceState) -> ResourceRecord {
    ResourceRecord {
        id: ResourceId::generate(),
        account_id,
        kind: BindingKind::VectorizeIndex,
        name: "direct".to_string(),
        state,
        availability: ResourceAvailability::Healthy,
        availability_code: None,
        spec_generation: 1,
        driver_schema_version: VECTORIZE_SCHEMA_VERSION,
        created_at_ms: 1,
        updated_at_ms: 1,
        deleted_at_ms: None,
    }
}

fn spec() -> VectorizeIndexSpec {
    VectorizeIndexSpec {
        dimensions: 32,
        metric: "cosine".to_string(),
        quota_vectors: 100,
        quota_bytes: 16 * 1024 * 1024,
    }
}

#[test]
fn public_resource_admission_rejects_dimensions_below_32() {
    let (_temporary, storage) = storage();
    let account = storage.identity().default_account_id;
    let controller = ResourceController::new(
        &storage,
        ResourcePins::new(),
        VectorizeResourceDriver::new(
            &storage,
            VectorizeIndexSpec {
                dimensions: 31,
                metric: "cosine".to_string(),
                quota_vectors: 100,
                quota_bytes: 16 * 1024 * 1024,
            },
            5_000,
        ),
    );
    assert_eq!(
        controller
            .create(&CreateResourceRequest {
                account_id: account,
                kind: BindingKind::VectorizeIndex,
                name: "too-small".to_string(),
                idempotency_key: "too-small".to_string(),
                driver_schema_version: VECTORIZE_SCHEMA_VERSION,
                request_id: RequestId::generate(),
                now_ms: 1,
            })
            .unwrap_err()
            .code(),
        ErrorCode::ResourceInvariantViolation
    );
}

#[test]
fn driver_rejects_every_invalid_frozen_spec_and_recovery_never_invents_one() {
    let (_temporary, storage) = storage();
    let account = storage.identity().default_account_id;
    let creating = record(account, ResourceState::Creating);
    for invalid_spec in [
        VectorizeIndexSpec {
            dimensions: 1_537,
            ..spec()
        },
        VectorizeIndexSpec {
            metric: "manhattan".to_string(),
            ..spec()
        },
        VectorizeIndexSpec {
            quota_vectors: 0,
            ..spec()
        },
        VectorizeIndexSpec {
            quota_vectors: 200_001,
            ..spec()
        },
        VectorizeIndexSpec {
            quota_bytes: 1_048_575,
            ..spec()
        },
    ] {
        let driver = VectorizeResourceDriver::new(&storage, invalid_spec, 5_000);
        assert_eq!(
            driver.create(&creating).unwrap_err().code(),
            ErrorCode::ResourceInvariantViolation
        );
    }

    let recovery = VectorizeResourceDriver::recovery(&storage, 5_000);
    assert_eq!(recovery.kind(), BindingKind::VectorizeIndex);
    assert!(recovery.create_fingerprint_material().is_empty());
    assert_eq!(
        recovery.create(&creating).unwrap_err().code(),
        ErrorCode::ResourceNotReady
    );
    assert_eq!(
        recovery.reconcile(&creating).unwrap(),
        ReconcileOutcome::Deferred
    );

    let driver = VectorizeResourceDriver::new(&storage, spec(), 5_000);
    assert!(!driver.create_fingerprint_material().is_empty());
    assert_eq!(
        driver.reconcile(&creating).unwrap(),
        ReconcileOutcome::Absent
    );
    let wrong_state = ResourceRecord {
        state: ResourceState::Ready,
        ..creating
    };
    assert_eq!(
        driver.create(&wrong_state).unwrap_err().code(),
        ErrorCode::ResourceInvariantViolation
    );
}

#[tokio::test]
async fn lifecycle_creates_health_checks_and_deletes_one_index() {
    let (_temporary, storage) = storage();
    let account = storage.identity().default_account_id;
    let pins = ResourcePins::new();
    let controller = ResourceController::new(
        &storage,
        pins,
        VectorizeResourceDriver::new(&storage, spec(), 5_000),
    );
    let resource_id = match controller
        .create(&CreateResourceRequest {
            account_id: account,
            kind: BindingKind::VectorizeIndex,
            name: "documents".to_string(),
            idempotency_key: "create-documents".to_string(),
            driver_schema_version: VECTORIZE_SCHEMA_VERSION,
            request_id: RequestId::generate(),
            now_ms: 10,
        })
        .unwrap()
    {
        CreateResourceOutcome::Applied(result) => result.resource_id,
        CreateResourceOutcome::Replay(_) => panic!("first create replayed"),
    };
    let record = VectorizeIndexRepository::new(storage.db())
        .get(account, resource_id)
        .unwrap();
    let path = VectorizePaths::open(storage.data_dir().root())
        .unwrap()
        .resolve_storage_key(&record.storage_key, account, resource_id)
        .unwrap();
    assert!(path.is_file());
    assert_eq!(
        controller
            .refresh_health(account, resource_id, 20)
            .unwrap()
            .availability,
        ResourceAvailability::Healthy
    );
    let driver = VectorizeResourceDriver::new(&storage, spec(), 5_000);
    let ready = ResourceRepository::new(storage.db())
        .get(account, resource_id)
        .unwrap();
    assert_eq!(driver.reconcile(&ready).unwrap(), ReconcileOutcome::Ready);
    assert_eq!(
        driver
            .reconcile(&ResourceRecord {
                state: ResourceState::Deleting,
                ..ready.clone()
            })
            .unwrap(),
        ReconcileOutcome::Ready
    );
    assert_eq!(
        driver.finalize_delete(&ready).unwrap_err().code(),
        ErrorCode::ResourceInvariantViolation
    );
    assert_eq!(
        driver
            .reconcile(&ResourceRecord {
                state: ResourceState::Tombstoned,
                ..ready.clone()
            })
            .unwrap(),
        ReconcileOutcome::Deleted
    );
    controller
        .delete(
            account,
            resource_id,
            RequestId::generate(),
            30,
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
    assert_eq!(
        driver
            .reconcile(&ResourceRecord {
                state: ResourceState::Deleting,
                ..ready.clone()
            })
            .unwrap(),
        ReconcileOutcome::Deleted
    );
    driver.begin_delete(&ready).unwrap();
    driver.finalize_delete(&ready).unwrap();
}
