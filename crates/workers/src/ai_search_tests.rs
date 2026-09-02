use super::*;
use crate::{CreateResourceOutcome, CreateResourceRequest, ResourceController, ResourcePins};
use open_compute_core::config::StorageConfig;
use open_compute_core::{ErrorCode, RequestId, SystemClock};
use open_compute_storage::ResourceRepository;
use sha2::{Digest as _, Sha256};
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

fn created_id(outcome: CreateResourceOutcome) -> ResourceId {
    match outcome {
        CreateResourceOutcome::Applied(result) => result.resource_id,
        CreateResourceOutcome::Replay(_) => panic!("first create replayed"),
    }
}

fn record(
    account_id: open_compute_core::AccountId,
    kind: BindingKind,
    state: ResourceState,
) -> ResourceRecord {
    ResourceRecord {
        id: ResourceId::generate(),
        account_id,
        kind,
        name: "direct".to_string(),
        state,
        availability: ResourceAvailability::Healthy,
        availability_code: None,
        spec_generation: 1,
        driver_schema_version: if kind == BindingKind::AiSearchNamespace {
            1
        } else {
            AI_SEARCH_SCHEMA_VERSION
        },
        created_at_ms: 1,
        updated_at_ms: 1,
        deleted_at_ms: None,
    }
}

fn spec(namespace_resource_id: ResourceId) -> AiSearchInstanceSpec {
    let public_config_json = br#"{"chunk":true,"chunk_overlap":15,"chunk_size":512,"custom_metadata":[],"fusion_method":"max","id":"docs","index_method":{"keyword":true,"vector":false},"max_num_results":10,"metadata":{},"score_threshold":0.3}"#.to_vec();
    let model_contract_json = br#"{"kind":"keyword_only","schemaVersion":1,"tokenizerContract":{"embeddingAlias":"@cf/qwen/qwen3-embedding-0.6b","tokenizer":"qwen3","tokenizerRevision":"fixture","tokenizerArtifactSha256":"def76fb086971c7867b829c23a26261e38d9d74e02139253b38aeb9df8b4b50a","maxInputTokens":8192,"contractSha256":"fixture"}}"#.to_vec();
    let model_contract_sha256 = Sha256::digest(&model_contract_json).into();
    AiSearchInstanceSpec {
        namespace_resource_id,
        instance_key: "docs".to_string(),
        public_config_json,
        model_contract_json,
        model_contract_sha256,
        dimensions: 0,
        vector_enabled: false,
        keyword_enabled: true,
    }
}

#[test]
fn direct_reconciliation_and_recovery_are_fail_closed() {
    let (_temporary, storage) = storage();
    let account = storage.identity().default_account_id;
    let namespace_creating = record(
        account,
        BindingKind::AiSearchNamespace,
        ResourceState::Creating,
    );
    let namespace_driver = AiSearchNamespaceResourceDriver::new(&storage);
    assert_eq!(namespace_driver.kind(), BindingKind::AiSearchNamespace);
    assert_eq!(
        namespace_driver.reconcile(&namespace_creating).unwrap(),
        ReconcileOutcome::Absent
    );
    let invalid_namespace = ResourceRecord {
        driver_schema_version: 2,
        ..namespace_creating.clone()
    };
    assert_eq!(
        namespace_driver
            .create(&invalid_namespace)
            .unwrap_err()
            .code(),
        ErrorCode::ResourceInvariantViolation
    );
    assert_eq!(
        namespace_driver
            .reconcile(&ResourceRecord {
                state: ResourceState::Deleting,
                ..namespace_creating.clone()
            })
            .unwrap(),
        ReconcileOutcome::Deleted
    );
    namespace_driver
        .finalize_delete(&namespace_creating)
        .unwrap();

    let instance_creating = record(
        account,
        BindingKind::AiSearchInstance,
        ResourceState::Creating,
    );
    let recovery = AiSearchInstanceResourceDriver::recovery(&storage, 5_000);
    assert_eq!(recovery.kind(), BindingKind::AiSearchInstance);
    assert!(recovery.create_fingerprint_material().is_empty());
    assert_eq!(
        recovery.create(&instance_creating).unwrap_err().code(),
        ErrorCode::ResourceNotReady
    );
    assert_eq!(
        recovery.reconcile(&instance_creating).unwrap(),
        ReconcileOutcome::Deferred
    );
    let driver = AiSearchInstanceResourceDriver::new(&storage, spec(namespace_creating.id), 5_000);
    assert!(!driver.create_fingerprint_material().is_empty());
    assert_eq!(
        driver.reconcile(&instance_creating).unwrap(),
        ReconcileOutcome::Absent
    );
    assert_eq!(
        driver
            .create(&ResourceRecord {
                state: ResourceState::Ready,
                ..instance_creating
            })
            .unwrap_err()
            .code(),
        ErrorCode::ResourceInvariantViolation
    );
}

#[tokio::test]
async fn namespace_and_instance_lifecycle_are_parent_scoped_and_recoverable() {
    let (_temporary, storage) = storage();
    let account = storage.identity().default_account_id;
    let pins = ResourcePins::new();
    let namespace = ResourceController::new(
        &storage,
        pins.clone(),
        AiSearchNamespaceResourceDriver::new(&storage),
    );
    let namespace_id = created_id(
        namespace
            .create(&CreateResourceRequest {
                account_id: account,
                kind: BindingKind::AiSearchNamespace,
                name: "knowledge".to_string(),
                idempotency_key: "namespace-create".to_string(),
                driver_schema_version: 1,
                request_id: RequestId::generate(),
                now_ms: 10,
            })
            .unwrap(),
    );

    let instance = ResourceController::new(
        &storage,
        pins,
        AiSearchInstanceResourceDriver::new(&storage, spec(namespace_id), 5_000),
    );
    let instance_id = created_id(
        instance
            .create(&CreateResourceRequest {
                account_id: account,
                kind: BindingKind::AiSearchInstance,
                name: "docs".to_string(),
                idempotency_key: "instance-create".to_string(),
                driver_schema_version: AI_SEARCH_SCHEMA_VERSION,
                request_id: RequestId::generate(),
                now_ms: 20,
            })
            .unwrap(),
    );
    let record = AiSearchCatalog::new(storage.db())
        .get_instance_by_key(account, namespace_id, "docs")
        .unwrap();
    assert_eq!(record.resource.id, instance_id);
    assert_eq!(
        instance
            .refresh_health(account, instance_id, 30)
            .unwrap()
            .availability,
        ResourceAvailability::Healthy
    );
    let driver = AiSearchInstanceResourceDriver::new(&storage, spec(namespace_id), 5_000);
    let ready = ResourceRepository::new(storage.db())
        .get(account, instance_id)
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
    assert!(
        namespace
            .delete(
                account,
                namespace_id,
                RequestId::generate(),
                40,
                Duration::from_secs(1),
            )
            .await
            .is_err()
    );
    instance
        .delete(
            account,
            instance_id,
            RequestId::generate(),
            50,
            Duration::from_secs(1),
        )
        .await
        .unwrap();
    namespace
        .delete(
            account,
            namespace_id,
            RequestId::generate(),
            60,
            Duration::from_secs(1),
        )
        .await
        .unwrap();
    assert_eq!(
        ResourceRepository::new(storage.db())
            .get(account, instance_id)
            .unwrap()
            .state,
        ResourceState::Tombstoned
    );
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
