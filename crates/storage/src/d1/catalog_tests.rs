use super::*;
use crate::{
    PlatformStorage, ReserveResourceCreate, ResourceCreateReservation, ResourceRepository,
};
use open_compute_core::config::StorageConfig;
use open_compute_core::{RequestId, SystemClock};

const QUOTA: u64 = 256 * 1024 * 1024;

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
    let fingerprint = storage.crypto().fingerprint_request(b"d1-catalog");
    let ResourceCreateReservation::Reserved(resource) = ResourceRepository::new(storage.db())
        .reserve_create(
            &ReserveResourceCreate {
                account_id: account,
                kind: BindingKind::D1Database,
                name: "catalog-db",
                idempotency_key: "catalog-db",
                fingerprint_key_id: storage.crypto().fingerprint_key_id(),
                request_fingerprint: &fingerprint,
                resource_id: ResourceId::generate(),
                driver_schema_version: 1,
                request_id: RequestId::generate(),
                now_ms: 10,
                expires_at_ms: 1_000,
            },
            1_000_000,
        )
        .unwrap()
    else {
        panic!("first reservation must create a resource");
    };
    (temp, storage, resource)
}

#[test]
fn database_catalog_validates_identity_updates_and_scope() {
    let (_temp, storage, resource) = fixture();
    let repository = D1DatabaseRepository::new(storage.db());
    let key = super::super::D1Paths::storage_key(resource.account_id, resource.id);
    let created = repository
        .ensure_database(&resource, &key, 1, QUOTA)
        .unwrap();
    assert_eq!(created.storage_key, key);
    assert_eq!(repository.list(resource.account_id).unwrap().len(), 1);
    repository.record_open(resource.id, 20).unwrap();
    repository.record_quick_check(resource.id, 21).unwrap();
    let updated = repository.get(resource.account_id, resource.id).unwrap();
    assert_eq!(updated.last_opened_at_ms, Some(20));
    assert_eq!(updated.last_quick_check_ms, Some(21));

    assert_eq!(
        repository
            .get(AccountId::generate(), resource.id)
            .unwrap_err()
            .code(),
        ErrorCode::ResourceNotFound
    );
    assert_eq!(
        repository
            .record_open(ResourceId::generate(), 30)
            .unwrap_err()
            .code(),
        ErrorCode::ResourceNotFound
    );
    assert_eq!(
        repository
            .ensure_database(&resource, &key, 0, QUOTA)
            .unwrap_err()
            .code(),
        ErrorCode::ResourceInvariantViolation
    );
    assert_eq!(
        repository
            .ensure_restoring_database(&resource, &key, 1, QUOTA, "not-a-uuid")
            .unwrap_err()
            .code(),
        ErrorCode::ResourceInvariantViolation
    );
}

#[test]
fn backup_catalog_covers_replay_failure_ready_and_tombstone_states() {
    let (_temp, storage, resource) = fixture();
    let repository = D1DatabaseRepository::new(storage.db());
    let key = super::super::D1Paths::storage_key(resource.account_id, resource.id);
    repository
        .ensure_database(&resource, &key, 1, QUOTA)
        .unwrap();
    for state in [
        D1BackupState::Creating,
        D1BackupState::Ready,
        D1BackupState::Failed,
        D1BackupState::Deleting,
        D1BackupState::Tombstoned,
    ] {
        assert_eq!(state.as_str().parse::<D1BackupState>().unwrap(), state);
    }
    assert_eq!(
        "invalid".parse::<D1BackupState>().unwrap_err().code(),
        ErrorCode::ResourceInvariantViolation
    );

    let failed_id = uuid::Uuid::now_v7().hyphenated().to_string();
    let fingerprint = storage.crypto().fingerprint_request(b"failed-backup");
    let reserved = repository
        .create_backup(resource.id, &failed_id, 1, 7, "failed", &fingerprint, 30)
        .unwrap();
    assert_eq!(reserved.state, D1BackupState::Creating);
    assert_eq!(
        repository
            .create_backup(resource.id, &failed_id, 1, 7, "failed", &fingerprint, 31)
            .unwrap()
            .id,
        failed_id
    );
    assert_eq!(
        repository
            .create_backup(resource.id, &failed_id, 1, 7, "failed", &[9; 32], 31)
            .unwrap_err()
            .code(),
        ErrorCode::IdempotencyConflict
    );
    let failed = repository
        .fail_backup(&failed_id, ErrorCode::S3Unavailable, 32)
        .unwrap();
    assert_eq!(failed.state, D1BackupState::Failed);
    assert_eq!(failed.error_code.as_deref(), Some("S3_UNAVAILABLE"));
    assert_eq!(
        repository
            .fail_backup(&failed_id, ErrorCode::Internal, 33)
            .unwrap_err()
            .code(),
        ErrorCode::ResourceInvariantViolation
    );

    let ready_id = uuid::Uuid::now_v7().hyphenated().to_string();
    let ready_fingerprint = storage.crypto().fingerprint_request(b"ready-backup");
    repository
        .create_backup(
            resource.id,
            &ready_id,
            1,
            8,
            "ready",
            &ready_fingerprint,
            40,
        )
        .unwrap();
    assert_eq!(
        repository
            .complete_backup(&ready_id, "bad-key", &[1; 32], 10, 41)
            .unwrap_err()
            .code(),
        ErrorCode::ResourceInvariantViolation
    );
    let ready = repository
        .complete_backup(
            &ready_id,
            &format!("system/backups/d1/{}/{ready_id}/data.sqlite", resource.id),
            &[1; 32],
            10,
            41,
        )
        .unwrap();
    assert_eq!(ready.state, D1BackupState::Ready);
    assert_eq!(
        repository
            .get_backup(resource.account_id, &ready_id)
            .unwrap(),
        ready
    );
    assert_eq!(
        repository
            .list_backups(resource.account_id, resource.id)
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        repository
            .get_backup(AccountId::generate(), &ready_id)
            .unwrap_err()
            .code(),
        ErrorCode::ResourceNotFound
    );
    assert_eq!(
        repository
            .tombstone_backup(AccountId::generate(), &ready_id, 42)
            .unwrap_err()
            .code(),
        ErrorCode::ResourceNotFound
    );
    let retired = repository
        .tombstone_backup(resource.account_id, &ready_id, 43)
        .unwrap();
    assert_eq!(retired.state, D1BackupState::Tombstoned);
    assert!(retired.sha256.is_none());

    for (id, schema, key) in [
        ("bad".to_owned(), 1, "x"),
        (uuid::Uuid::now_v7().hyphenated().to_string(), 0, "x"),
        (uuid::Uuid::now_v7().hyphenated().to_string(), 1, ""),
    ] {
        assert_eq!(
            repository
                .create_backup(resource.id, &id, schema, 0, key, &[0; 32], 50)
                .unwrap_err()
                .code(),
            ErrorCode::ResourceInvariantViolation
        );
    }
}
