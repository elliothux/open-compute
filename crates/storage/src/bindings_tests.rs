use super::*;
use crate::{
    NewQueueProducerBinding, NewVersion, PlatformStorage, QueueConfig, QueueRepository,
    ReserveResourceCreate, ResourceCreateReservation, ResourceRepository, WorkerRepository,
};
use open_compute_core::config::DataConfig;
use open_compute_core::{BindingId, QueueId, RequestId, SystemClock, WorkerId};
use std::collections::BTreeMap;

fn storage() -> (tempfile::TempDir, PlatformStorage) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("data");
    let config = DataConfig {
        path: root.clone(),
        master_key_file: root.join("keys/master.key"),
        master_key_env: None,
        sqlite_busy_timeout_ms: 5_000,
        free_space_soft_bytes: 1_073_741_824,
        free_space_hard_bytes: 268_435_456,
    };
    let storage = PlatformStorage::bootstrap(&config, &SystemClock).unwrap();
    (temp, storage)
}

fn version(account_id: AccountId, worker_id: WorkerId, version_id: VersionId) -> NewVersion {
    NewVersion {
        id: version_id,
        account_id,
        worker_id,
        content_kind: crate::VersionContentKind::Worker,
        artifact_sha256: Some([1; 32]),
        artifact_size: Some(1),
        artifact_schema_version: Some(1),
        main_module: Some("index.js".to_owned()),
        worker_code_sha256: [2; 32],
        compatibility_date: "2026-08-30".into(),
        compatibility_flags: Vec::new(),
        vars: BTreeMap::new(),
        secrets: BTreeMap::new(),
        request_id: RequestId::generate(),
        now_ms: 20,
    }
}

#[test]
fn binding_insert_referrer_authorize_and_worker_release_are_atomic() {
    let (_temp, storage) = storage();
    let account = storage.identity().default_account_id;
    let resources = ResourceRepository::new(storage.db());
    let resource_id = ResourceId::generate();
    let fingerprint = [3; 32];
    let reserved = resources
        .reserve_create(
            &ReserveResourceCreate {
                account_id: account,
                kind: BindingKind::KvNamespace,
                name: "cache",
                idempotency_key: "resource",
                fingerprint_key_id: "key",
                request_fingerprint: &fingerprint,
                resource_id,
                driver_schema_version: 1,
                request_id: RequestId::generate(),
                now_ms: 10,
                expires_at_ms: 100,
            },
            1_000_000,
        )
        .unwrap();
    assert!(matches!(reserved, ResourceCreateReservation::Reserved(_)));
    resources.mark_ready(resource_id, 11).unwrap();

    let workers = WorkerRepository::new(storage.db());
    let (worker, _) = workers
        .create_worker(account, "bound", RequestId::generate(), 12, 1_000_000)
        .unwrap();
    let version_id = VersionId::generate();
    let binding_id = BindingId::generate();
    let descriptor = [9; 32];
    let binding = NewVersionBinding {
        id: binding_id,
        name: "CACHE".to_owned(),
        kind: BindingKind::KvNamespace,
        resource_id,
        resource_spec_generation: 1,
        capability_version: 1,
        permissions_json: br#"{"read":true,"write":false}"#.to_vec(),
        config_json: b"{}".to_vec(),
        descriptor_sha256: descriptor,
    };
    workers
        .insert_staging_version(
            &version(account, worker.id, version_id),
            &crate::NewVersionProducts {
                bindings: std::slice::from_ref(&binding),
                ..Default::default()
            },
            1_000_000,
        )
        .unwrap();
    assert_eq!(resources.referrers(resource_id).unwrap().len(), 1);
    assert_eq!(
        resources
            .begin_delete(account, resource_id, 21)
            .unwrap_err()
            .code(),
        ErrorCode::ResourceReferenced
    );
    workers.begin_validation(version_id).unwrap();
    workers.mark_ready(version_id, 22).unwrap();
    let authorized = BindingRepository::new(storage.db())
        .authorize(binding_id, version_id, &descriptor)
        .unwrap();
    assert_eq!(authorized.resource.id, resource_id);
    assert!(!authorized.binding.permissions.write);

    workers
        .delete_worker(account, worker.id, &[version_id], RequestId::generate(), 23)
        .unwrap();
    assert!(resources.referrers(resource_id).unwrap().is_empty());
    resources.begin_delete(account, resource_id, 24).unwrap();
}

#[test]
fn queue_producer_referrer_is_released_when_its_worker_is_deleted() {
    let (_temp, storage) = storage();
    let account = storage.identity().default_account_id;
    let queues = QueueRepository::new(storage.db());
    let queue_id = QueueId::generate();
    queues
        .insert_creating(account, queue_id, "events", QueueConfig::default(), 10)
        .unwrap();
    queues.mark_ready(account, queue_id, 11).unwrap();

    let workers = WorkerRepository::new(storage.db());
    let (worker, _) = workers
        .create_worker(account, "queue-bound", RequestId::generate(), 12, 1_000_000)
        .unwrap();
    let version_id = VersionId::generate();
    let binding = NewQueueProducerBinding {
        id: BindingId::generate(),
        name: "EVENTS".to_owned(),
        queue_id,
        queue_lifecycle_generation: 1,
        capability_version: 1,
        descriptor_sha256: [3; 32],
    };
    workers
        .insert_staging_version(
            &version(account, worker.id, version_id),
            &crate::NewVersionProducts {
                queue_bindings: std::slice::from_ref(&binding),
                ..Default::default()
            },
            1_000_000,
        )
        .unwrap();
    workers.begin_validation(version_id).unwrap();
    workers.mark_ready(version_id, 13).unwrap();
    assert_eq!(
        queues
            .begin_delete(account, queue_id, 1, 14)
            .unwrap_err()
            .code(),
        ErrorCode::QueueReferenced
    );

    workers
        .delete_worker(account, worker.id, &[version_id], RequestId::generate(), 15)
        .unwrap();
    queues.begin_delete(account, queue_id, 1, 16).unwrap();
}

#[test]
fn binding_triggers_reject_cross_kind_and_runtime_forgery() {
    let (_temp, storage) = storage();
    let account = storage.identity().default_account_id;
    let resources = ResourceRepository::new(storage.db());
    let resource_id = ResourceId::generate();
    let fingerprint = [4; 32];
    resources
        .reserve_create(
            &ReserveResourceCreate {
                account_id: account,
                kind: BindingKind::KvNamespace,
                name: "cache",
                idempotency_key: "resource",
                fingerprint_key_id: "key",
                request_fingerprint: &fingerprint,
                resource_id,
                driver_schema_version: 1,
                request_id: RequestId::generate(),
                now_ms: 10,
                expires_at_ms: 100,
            },
            1_000_000,
        )
        .unwrap();
    resources.mark_ready(resource_id, 11).unwrap();
    let workers = WorkerRepository::new(storage.db());
    let (worker, _) = workers
        .create_worker(account, "bound", RequestId::generate(), 12, 1_000_000)
        .unwrap();
    let version_id = VersionId::generate();
    let binding_id = BindingId::generate();
    let bad = NewVersionBinding {
        id: binding_id,
        name: "CACHE".to_owned(),
        kind: BindingKind::R2Bucket,
        resource_id,
        resource_spec_generation: 1,
        capability_version: 1,
        permissions_json: br#"{"read":true,"write":true}"#.to_vec(),
        config_json: b"{}".to_vec(),
        descriptor_sha256: [5; 32],
    };
    assert_eq!(
        workers
            .insert_staging_version(
                &version(account, worker.id, version_id),
                &crate::NewVersionProducts {
                    bindings: &[bad],
                    ..Default::default()
                },
                1_000_000
            )
            .unwrap_err()
            .code(),
        ErrorCode::BindingTypeMismatch
    );
    assert_eq!(
        BindingRepository::new(storage.db())
            .authorize(binding_id, version_id, &[5; 32])
            .unwrap_err()
            .code(),
        ErrorCode::BindingNotFound
    );
}
