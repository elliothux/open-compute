use super::*;
use crate::{
    PlatformStorage, ReserveResourceCreate, ResourceCreateReservation, ResourceRepository,
};
use open_compute_core::config::StorageConfig;
use open_compute_core::{BindingKind, RequestId, SystemClock};

fn fixture() -> (tempfile::TempDir, PlatformStorage, ResourceRecord) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("data");
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
    let resource_id = ResourceId::generate();
    let fingerprint = storage.crypto().fingerprint_request(b"r2-catalog-test");
    let reserved = ResourceRepository::new(storage.db())
        .reserve_create(
            &ReserveResourceCreate {
                account_id: storage.identity().default_account_id,
                kind: BindingKind::R2Bucket,
                name: "images",
                idempotency_key: "r2-catalog-test",
                fingerprint_key_id: storage.crypto().fingerprint_key_id(),
                request_fingerprint: &fingerprint,
                resource_id,
                driver_schema_version: R2_SCHEMA_VERSION,
                request_id: RequestId::generate(),
                now_ms: 10,
                expires_at_ms: 1_000,
            },
            1_000_000,
        )
        .unwrap();
    let ResourceCreateReservation::Reserved(resource) = reserved else {
        unreachable!()
    };
    (temp, storage, resource)
}

#[test]
fn locator_is_immutable_scoped_and_not_serialized() {
    let (_temp, storage, resource) = fixture();
    let repo = R2BucketRepository::new(storage.db());
    let prefix = format!("tenant/r2/v1/{}/", resource.id);
    let authority = [1_u8; 32];
    let record = repo
        .ensure_bucket(&resource, &prefix, 512 * 1024 * 1024, &authority)
        .unwrap();
    assert_eq!(record.physical_prefix, prefix);
    assert_eq!(repo.get(resource.account_id, resource.id).unwrap(), record);
    assert_eq!(
        repo.list(resource.account_id).unwrap(),
        vec![record.clone()]
    );
    assert_eq!(repo.list_all().unwrap(), vec![record.clone()]);
    let serialized = serde_json::to_string(&record).unwrap();
    assert!(!serialized.contains("tenant/r2"));
    assert!(!serialized.contains("providerConfig"));
    assert_eq!(
        repo.get(AccountId::generate(), resource.id)
            .unwrap_err()
            .code(),
        ErrorCode::ResourceNotFound
    );
    assert_eq!(
        repo.ensure_bucket(&resource, "bad", 1, &authority)
            .unwrap_err()
            .code(),
        ErrorCode::ResourceInvariantViolation
    );
}

#[test]
fn locator_retires_only_after_deletion_fence_and_tombstone() {
    let (_temp, storage, resource) = fixture();
    let buckets = R2BucketRepository::new(storage.db());
    let resources = ResourceRepository::new(storage.db());
    buckets
        .ensure_bucket(
            &resource,
            &format!("tenant/r2/v1/{}/", resource.id),
            1024,
            &[1_u8; 32],
        )
        .unwrap();
    resources.mark_ready(resource.id, 11).unwrap();
    resources
        .begin_delete(resource.account_id, resource.id, 12)
        .unwrap();
    assert!(
        resources
            .mark_tombstoned(resource.account_id, resource.id, RequestId::generate(), 13)
            .is_err()
    );
    buckets.mark_delete_started(resource.id, 14).unwrap();
    resources
        .mark_tombstoned(resource.account_id, resource.id, RequestId::generate(), 15)
        .unwrap();
    assert_eq!(
        buckets
            .get(resource.account_id, resource.id)
            .unwrap_err()
            .code(),
        ErrorCode::ResourceNotFound
    );
}
