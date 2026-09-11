use super::super::*;
use crate::{
    PlatformStorage, ReserveResourceCreate, ResourceCreateReservation, ResourceRecord,
    ResourceRepository,
};
use open_compute_core::config::DataConfig;
use open_compute_core::{BindingKind, ErrorCode, RequestId, ResourceId, SystemClock};

fn fixture() -> (tempfile::TempDir, PlatformStorage) {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join("data");
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
    (temporary, storage)
}

fn reserve(storage: &PlatformStorage, kind: BindingKind, name: &str) -> ResourceRecord {
    let account_id = storage.identity().default_account_id;
    let fingerprint = storage.crypto().fingerprint_request(name.as_bytes());
    let reservation = ResourceRepository::new(storage.db())
        .reserve_create(
            &ReserveResourceCreate {
                account_id,
                kind,
                name,
                idempotency_key: name,
                fingerprint_key_id: storage.crypto().fingerprint_key_id(),
                request_fingerprint: &fingerprint,
                resource_id: ResourceId::generate(),
                driver_schema_version: if kind == BindingKind::AiSearchInstance {
                    AI_SEARCH_SCHEMA_VERSION
                } else {
                    1
                },
                request_id: RequestId::generate(),
                now_ms: 10,
                expires_at_ms: 1_000,
            },
            100,
        )
        .unwrap();
    let ResourceCreateReservation::Reserved(resource) = reservation else {
        unreachable!()
    };
    resource
}

#[test]
fn catalog_enforces_parent_scope_and_tracks_instance_lifecycle() {
    let (_temporary, storage) = fixture();
    let resources = ResourceRepository::new(storage.db());
    let catalog = AiSearchCatalog::new(storage.db());
    let namespace = reserve(&storage, BindingKind::AiSearchNamespace, "documents");
    assert_eq!(
        catalog.ensure_namespace(&namespace).unwrap().resource.id,
        namespace.id
    );
    resources.mark_ready(namespace.id, 20).unwrap();

    let instance = reserve(&storage, BindingKind::AiSearchInstance, "primary");
    assert_eq!(
        catalog
            .ensure_instance(
                &instance,
                namespace.id,
                "Invalid",
                "storage",
                AI_SEARCH_SCHEMA_VERSION,
                [7; 32],
            )
            .unwrap_err()
            .code(),
        ErrorCode::ConfigInvalid
    );
    let inserted = catalog
        .ensure_instance(
            &instance,
            namespace.id,
            "primary_v1",
            "ai-search/v1/primary",
            AI_SEARCH_SCHEMA_VERSION,
            [7; 32],
        )
        .unwrap();
    assert_eq!(
        catalog
            .get_instance(instance.account_id, instance.id)
            .unwrap(),
        inserted
    );
    assert_eq!(
        catalog
            .get_instance_by_key(instance.account_id, namespace.id, "primary_v1")
            .unwrap(),
        inserted
    );
    assert_eq!(
        catalog
            .list_instances(instance.account_id, namespace.id)
            .unwrap(),
        vec![inserted]
    );
    assert!(
        catalog
            .has_live_instances(instance.account_id, namespace.id)
            .unwrap()
    );
    assert!(catalog.list_ready_instances().unwrap().is_empty());

    resources.mark_ready(instance.id, 21).unwrap();
    assert_eq!(catalog.list_ready_instances().unwrap().len(), 1);
    assert!(
        catalog
            .update_model_contract(instance.account_id, instance.id, [7; 32], [8; 32])
            .unwrap()
    );
    assert!(
        !catalog
            .update_model_contract(instance.account_id, instance.id, [7; 32], [9; 32])
            .unwrap()
    );
    assert_eq!(
        catalog
            .get_instance(instance.account_id, instance.id)
            .unwrap()
            .model_contract_sha256,
        [8; 32]
    );

    resources
        .begin_delete(instance.account_id, instance.id, 22)
        .unwrap();
    assert!(catalog.list_ready_instances().unwrap().is_empty());
    assert_eq!(catalog.list_deleting_instances().unwrap().len(), 1);
    resources
        .mark_tombstoned(instance.account_id, instance.id, RequestId::generate(), 23)
        .unwrap();
    assert!(
        !catalog
            .has_live_instances(instance.account_id, namespace.id)
            .unwrap()
    );
    assert!(
        catalog
            .list_instances(instance.account_id, namespace.id)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn r2_source_identity_is_frozen_and_blocks_bucket_deletion() {
    let (_temporary, storage) = fixture();
    let resources = ResourceRepository::new(storage.db());
    let namespace = reserve(&storage, BindingKind::AiSearchNamespace, "source-namespace");
    AiSearchCatalog::new(storage.db())
        .ensure_namespace(&namespace)
        .unwrap();
    resources.mark_ready(namespace.id, 20).unwrap();

    let bucket = reserve(&storage, BindingKind::R2Bucket, "documents");
    let prefix = format!("r2/v1/{}/", bucket.id);
    crate::R2BucketRepository::new(storage.db())
        .ensure_bucket(&bucket, &prefix, 1024, &[9; 32])
        .unwrap();
    resources.mark_ready(bucket.id, 21).unwrap();

    let instance = reserve(&storage, BindingKind::AiSearchInstance, "r2-instance");
    let inserted = AiSearchCatalog::new(storage.db())
        .ensure_instance_with_r2_source(
            &instance,
            namespace.id,
            "r2_instance",
            "ai-search/v2/r2-instance",
            AI_SEARCH_SCHEMA_VERSION,
            [7; 32],
            Some((bucket.id, "documents")),
        )
        .unwrap();
    let source = inserted.r2_source.unwrap();
    assert_eq!(source.bucket_resource_id, bucket.id);
    assert_eq!(source.bucket_name, "documents");
    assert_eq!(
        resources.referrers(bucket.id).unwrap()[0].referrer_kind,
        "ai_search_r2_source"
    );
    assert_eq!(
        resources
            .begin_delete(bucket.account_id, bucket.id, 30)
            .unwrap_err()
            .code(),
        ErrorCode::ResourceReferenced
    );
}
