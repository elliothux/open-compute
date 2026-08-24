use super::*;
use crate::{
    NewDeployment, PlatformStorage, ReserveResourceCreate, ResourceCreateReservation,
    ResourceRepository, WorkerRepository,
};
use open_compute_core::config::StorageConfig;
use open_compute_core::{RequestId, SystemClock, WorkerId};
use std::collections::BTreeMap;

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

fn deployment(
    account_id: AccountId,
    worker_id: WorkerId,
    deployment_id: DeploymentId,
) -> NewDeployment {
    NewDeployment {
        id: deployment_id,
        account_id,
        worker_id,
        artifact_sha256: [1; 32],
        artifact_size: 1,
        artifact_schema_version: 1,
        main_module: "index.js".to_owned(),
        compatibility_date: "2026-08-22".to_owned(),
        compatibility_flags: Vec::new(),
        limits: serde_json::json!({"profile":"default"}),
        worker_code_sha256: [2; 32],
        vars: BTreeMap::new(),
        secrets: BTreeMap::new(),
        request_id: RequestId::generate(),
        now_ms: 20,
    }
}

#[test]
fn binding_insert_referrer_authorize_and_release_are_atomic() {
    let (_temp, storage) = storage();
    let account = storage.identity().default_account_id;
    let resources = ResourceRepository::new(storage.db());
    let resource_id = ResourceId::generate();
    let fingerprint = [3; 32];
    let reserved = resources
        .reserve_create(&ReserveResourceCreate {
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
        })
        .unwrap();
    assert!(matches!(reserved, ResourceCreateReservation::Reserved(_)));
    resources.mark_ready(resource_id, 11).unwrap();

    let workers = WorkerRepository::new(storage.db());
    let (worker, _) = workers
        .create_worker(account, "bound", RequestId::generate(), 12)
        .unwrap();
    let deployment_id = DeploymentId::generate();
    let binding_id = BindingId::generate();
    let descriptor = [9; 32];
    let binding = NewDeploymentBinding {
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
        .insert_staging_deployment_with_bindings(
            &deployment(account, worker.id, deployment_id),
            std::slice::from_ref(&binding),
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
    workers.begin_validation(deployment_id).unwrap();
    workers.mark_ready(deployment_id, 22).unwrap();
    let authorized = BindingRepository::new(storage.db())
        .authorize(binding_id, deployment_id, &descriptor)
        .unwrap();
    assert_eq!(authorized.resource.id, resource_id);
    assert!(!authorized.binding.permissions.write);

    workers
        .tombstone_deployment(account, worker.id, deployment_id, RequestId::generate(), 23)
        .unwrap();
    assert!(resources.referrers(resource_id).unwrap().is_empty());
}

#[test]
fn binding_triggers_reject_cross_kind_and_runtime_forgery() {
    let (_temp, storage) = storage();
    let account = storage.identity().default_account_id;
    let resources = ResourceRepository::new(storage.db());
    let resource_id = ResourceId::generate();
    let fingerprint = [4; 32];
    resources
        .reserve_create(&ReserveResourceCreate {
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
        })
        .unwrap();
    resources.mark_ready(resource_id, 11).unwrap();
    let workers = WorkerRepository::new(storage.db());
    let (worker, _) = workers
        .create_worker(account, "bound", RequestId::generate(), 12)
        .unwrap();
    let deployment_id = DeploymentId::generate();
    let binding_id = BindingId::generate();
    let bad = NewDeploymentBinding {
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
            .insert_staging_deployment_with_bindings(
                &deployment(account, worker.id, deployment_id),
                &[bad],
            )
            .unwrap_err()
            .code(),
        ErrorCode::BindingTypeMismatch
    );
    assert_eq!(
        BindingRepository::new(storage.db())
            .authorize(binding_id, deployment_id, &[5; 32])
            .unwrap_err()
            .code(),
        ErrorCode::BindingNotFound
    );
}
