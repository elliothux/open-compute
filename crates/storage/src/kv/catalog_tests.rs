use super::*;
use crate::{
    KvPaths, PlatformStorage, ReserveResourceCreate, ResourceCreateReservation, ResourceRepository,
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
    let account = storage.identity().default_account_id;
    let resource_id = ResourceId::generate();
    let fingerprint = storage.crypto().fingerprint_request(b"kv-catalog-test");
    let reserved = ResourceRepository::new(storage.db())
        .reserve_create(
            &ReserveResourceCreate {
                account_id: account,
                kind: BindingKind::KvNamespace,
                name: "cache",
                idempotency_key: "kv-catalog-test",
                fingerprint_key_id: storage.crypto().fingerprint_key_id(),
                request_fingerprint: &fingerprint,
                resource_id,
                driver_schema_version: 1,
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
fn namespace_catalog_round_trips_and_conceals_physical_locator() {
    let (_temp, storage, resource) = fixture();
    let repo = KvNamespaceRepository::new(storage.db());
    assert!(repo.list(resource.account_id).unwrap().is_empty());
    assert_eq!(
        repo.ensure_namespace(&resource, "bad", 0, 1)
            .unwrap_err()
            .code(),
        ErrorCode::ResourceInvariantViolation
    );

    let key = KvPaths::storage_key(resource.account_id, resource.id);
    let inserted = repo
        .ensure_namespace(&resource, &key, 1, 256 * 1024 * 1024)
        .unwrap();
    assert_eq!(inserted.storage_key, key);
    assert_eq!(
        repo.get(resource.account_id, resource.id).unwrap(),
        inserted
    );
    assert_eq!(
        repo.list(resource.account_id).unwrap(),
        vec![inserted.clone()]
    );
    assert!(
        serde_json::to_string(&inserted)
            .unwrap()
            .contains("quotaBytes")
    );
    assert!(
        !serde_json::to_string(&inserted)
            .unwrap()
            .contains("storage_key")
    );

    repo.record_open(resource.id, 20).unwrap();
    repo.record_quick_check(resource.id, 21).unwrap();
    repo.record_backup(resource.id, 22).unwrap();
    let updated = repo.get(resource.account_id, resource.id).unwrap();
    assert_eq!(updated.last_opened_at_ms, Some(20));
    assert_eq!(updated.last_quick_check_ms, Some(21));
    assert_eq!(updated.last_backup_at_ms, Some(22));
    assert_eq!(
        repo.get(AccountId::generate(), resource.id)
            .unwrap_err()
            .code(),
        ErrorCode::ResourceNotFound
    );
    assert_eq!(
        repo.record_open(ResourceId::generate(), 1)
            .unwrap_err()
            .code(),
        ErrorCode::ResourceNotFound
    );
}

#[test]
fn backup_catalog_enforces_state_scope_and_immutable_fields() {
    let (_temp, storage, resource) = fixture();
    let repo = KvNamespaceRepository::new(storage.db());
    let key = KvPaths::storage_key(resource.account_id, resource.id);
    repo.ensure_namespace(&resource, &key, 1, 256 * 1024 * 1024)
        .unwrap();

    for state in [
        KvBackupState::Creating,
        KvBackupState::Ready,
        KvBackupState::Failed,
        KvBackupState::Deleting,
        KvBackupState::Tombstoned,
    ] {
        assert_eq!(KvBackupState::from_str(state.as_str()).unwrap(), state);
    }
    assert!(KvBackupState::from_str("unknown").is_err());
    assert!(
        repo.create_backup(resource.id, "not-a-uuid", 1, "bad", &[1; 32], 30)
            .is_err()
    );

    let ready_id = uuid::Uuid::now_v7().hyphenated().to_string();
    let creating = repo
        .create_backup(resource.id, &ready_id, 1, "ready", &[2; 32], 30)
        .unwrap();
    assert_eq!(creating.state, KvBackupState::Creating);
    assert_eq!(
        repo.create_backup(
            resource.id,
            &uuid::Uuid::now_v7().hyphenated().to_string(),
            1,
            "ready",
            &[2; 32],
            31,
        )
        .unwrap()
        .id,
        ready_id
    );
    assert_eq!(
        repo.create_backup(
            resource.id,
            &uuid::Uuid::now_v7().hyphenated().to_string(),
            1,
            "ready",
            &[9; 32],
            31,
        )
        .unwrap_err()
        .code(),
        ErrorCode::IdempotencyConflict
    );
    assert!(
        repo.complete_backup(&ready_id, "bad", &[7; 32], 9, 31)
            .is_err()
    );
    let ready = repo
        .complete_backup(
            &ready_id,
            "system/backups/kv/a/r/b/data.sqlite",
            &[7; 32],
            9,
            31,
        )
        .unwrap();
    assert_eq!(ready.state, KvBackupState::Ready);
    assert_eq!(ready.sha256, Some([7; 32]));
    assert_eq!(ready.size_bytes, Some(9));
    assert_eq!(
        repo.get_backup(AccountId::generate(), &ready_id)
            .unwrap_err()
            .code(),
        ErrorCode::ResourceNotFound
    );

    let failed_id = uuid::Uuid::now_v7().hyphenated().to_string();
    repo.create_backup(resource.id, &failed_id, 1, "failed", &[3; 32], 32)
        .unwrap();
    let failed = repo
        .fail_backup(&failed_id, ErrorCode::KvUnavailable, 33)
        .unwrap();
    assert_eq!(failed.state, KvBackupState::Failed);
    assert_eq!(failed.error_code.as_deref(), Some("KV_UNAVAILABLE"));
    assert!(
        repo.fail_backup(&failed_id, ErrorCode::Internal, 34)
            .is_err()
    );

    assert_eq!(repo.list_backups(resource.account_id).unwrap().len(), 2);
    let tombstone = repo
        .tombstone_backup(resource.account_id, &ready_id, 40)
        .unwrap();
    assert_eq!(tombstone.state, KvBackupState::Tombstoned);
    assert!(tombstone.object_key.is_none());
    assert!(tombstone.sha256.is_none());
    assert!(
        repo.tombstone_backup(resource.account_id, &ready_id, 41)
            .is_err()
    );
    assert_eq!(
        repo.tombstone_backup(AccountId::generate(), &failed_id, 42)
            .unwrap_err()
            .code(),
        ErrorCode::ResourceNotFound
    );
}
