use super::*;
use crate::{PlatformStorage, inspect_resources};
use open_compute_core::SystemClock;
use open_compute_core::config::StorageConfig;

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

fn reserve<'a>(
    account_id: AccountId,
    name: &'a str,
    key: &'a str,
    fingerprint: &'a [u8; 32],
    resource_id: ResourceId,
) -> ReserveResourceCreate<'a> {
    ReserveResourceCreate {
        account_id,
        kind: BindingKind::KvNamespace,
        name,
        idempotency_key: key,
        fingerprint_key_id: "fingerprint-key",
        request_fingerprint: fingerprint,
        resource_id,
        driver_schema_version: 1,
        request_id: RequestId::generate(),
        now_ms: 10,
        expires_at_ms: 100,
    }
}

#[test]
fn create_replay_rename_health_delete_and_same_name_recreate() {
    let (_temp, storage) = storage();
    let account = storage.identity().default_account_id;
    let repo = ResourceRepository::new(storage.db());
    let fingerprint = [7; 32];
    let first_id = ResourceId::generate();
    let input = reserve(account, "cache", "create-cache", &fingerprint, first_id);
    let first = match repo.reserve_create(&input, 1_000_000).unwrap() {
        ResourceCreateReservation::Reserved(record) => record,
        other => panic!("unexpected {other:?}"),
    };
    assert_eq!(first.state, ResourceState::Creating);
    assert!(matches!(
        repo.reserve_create(&input, 1_000_000).unwrap(),
        ResourceCreateReservation::Continue(record) if record.id == first_id
    ));
    repo.mark_ready(first_id, 11).unwrap();
    let renamed = repo
        .rename(account, first_id, "renamed", RequestId::generate(), 12)
        .unwrap();
    assert_eq!(renamed.name, "renamed");
    assert_eq!(renamed.spec_generation, 1);
    assert_eq!(
        repo.set_availability(
            account,
            first_id,
            ResourceAvailability::Unavailable,
            Some("TEST_UNAVAILABLE"),
            13,
        )
        .unwrap()
        .state,
        ResourceState::Ready
    );
    repo.set_availability(account, first_id, ResourceAvailability::Healthy, None, 14)
        .unwrap();
    repo.begin_delete(account, first_id, 15).unwrap();
    repo.mark_tombstoned(account, first_id, RequestId::generate(), 16)
        .unwrap();
    assert_eq!(
        repo.get(account, first_id).unwrap().state,
        ResourceState::Tombstoned
    );

    let second_id = ResourceId::generate();
    let second_fingerprint = [8; 32];
    let second = reserve(
        account,
        "renamed",
        "create-cache-again",
        &second_fingerprint,
        second_id,
    );
    assert!(matches!(
        repo.reserve_create(&second, 1_000_000).unwrap(),
        ResourceCreateReservation::Reserved(record) if record.id == second_id
    ));
    assert_ne!(first_id, second_id);
}

#[test]
fn resource_repository_fails_closed_on_conflicts_scope_and_invalid_transitions() {
    let (_temp, storage) = storage();
    let account = storage.identity().default_account_id;
    let repo = ResourceRepository::new(storage.db());
    let fingerprint = [1; 32];
    let id = ResourceId::generate();
    let input = reserve(account, "cache", "key", &fingerprint, id);
    repo.reserve_create(&input, 1_000_000).unwrap();
    assert_eq!(
        repo.reserve_create(
            &ReserveResourceCreate {
                request_fingerprint: &[2; 32],
                ..input.clone()
            },
            1_000_000
        )
        .unwrap_err()
        .code(),
        ErrorCode::IdempotencyConflict
    );
    assert_eq!(
        repo.get(AccountId::generate(), id).unwrap_err().code(),
        ErrorCode::ResourceNotFound
    );
    assert_eq!(
        repo.set_availability(account, id, ResourceAvailability::Unavailable, None, 11,)
            .unwrap_err()
            .code(),
        ErrorCode::ResourceInvariantViolation
    );
    repo.mark_ready(id, 12).unwrap();
    assert_eq!(repo.reconcile_candidates().unwrap().len(), 0);
    assert_eq!(
        repo.list(account, Some(BindingKind::KvNamespace))
            .unwrap()
            .len(),
        1
    );
    repo.complete_create(account, "key", &fingerprint, id, b"done")
        .unwrap();
    assert!(matches!(
        repo.reserve_create(&input, 1_000_000).unwrap(),
        ResourceCreateReservation::Complete(body) if body == b"done"
    ));
    assert_eq!(
        repo.complete_create(account, "key", &fingerprint, id, b"again")
            .unwrap_err()
            .code(),
        ErrorCode::IdempotencyConflict
    );
}

#[test]
fn create_input_bounds_fail_before_persistence() {
    let (_temp, storage) = storage();
    let account = storage.identity().default_account_id;
    let repo = ResourceRepository::new(storage.db());
    let fingerprint = [3; 32];

    for name in ["", "bad\nname"] {
        assert_eq!(
            repo.reserve_create(
                &reserve(
                    account,
                    name,
                    "valid-key",
                    &fingerprint,
                    ResourceId::generate(),
                ),
                1_000_000
            )
            .unwrap_err()
            .code(),
            ErrorCode::ResourceInvariantViolation
        );
    }
    for key in ["", "bad key"] {
        assert_eq!(
            repo.reserve_create(
                &reserve(
                    account,
                    "valid-name",
                    key,
                    &fingerprint,
                    ResourceId::generate(),
                ),
                1_000_000
            )
            .unwrap_err()
            .code(),
            ErrorCode::IdempotencyConflict
        );
    }
    let mut invalid_schema = reserve(
        account,
        "valid-name",
        "valid-key",
        &fingerprint,
        ResourceId::generate(),
    );
    invalid_schema.driver_schema_version = 0;
    assert_eq!(
        repo.reserve_create(&invalid_schema, 1_000_000)
            .unwrap_err()
            .code(),
        ErrorCode::ResourceInvariantViolation
    );
}

#[test]
fn read_only_inspection_lists_only_secret_free_resource_health() {
    let (_temp, storage) = storage();
    let account = storage.identity().default_account_id;
    let id = ResourceId::generate();
    let fingerprint = [4; 32];
    let repo = ResourceRepository::new(storage.db());
    repo.reserve_create(
        &reserve(account, "inspect", "inspect", &fingerprint, id),
        1_000_000,
    )
    .unwrap();
    repo.mark_ready(id, 11).unwrap();
    repo.set_availability(
        account,
        id,
        ResourceAvailability::Unavailable,
        Some("FAKE_UNAVAILABLE"),
        12,
    )
    .unwrap();
    let path = storage.data_dir().control_db_path();
    drop(storage);
    let rows = inspect_resources(&path, 5_000, 10).unwrap();
    assert_eq!(
        rows,
        vec![crate::ResourceInspect {
            id,
            kind: BindingKind::KvNamespace,
            availability: ResourceAvailability::Unavailable,
            availability_code: Some("FAKE_UNAVAILABLE".to_owned()),
        }]
    );
}

#[test]
fn delete_reservations_replay_continue_complete_and_reject_conflicts() {
    let (_temp, storage) = storage();
    let account = storage.identity().default_account_id;
    let repository = ResourceRepository::new(storage.db());
    let resource_id = ResourceId::generate();
    repository
        .reserve_create(
            &reserve(
                account,
                "delete-me",
                "create-delete-me",
                &[1; 32],
                resource_id,
            ),
            100,
        )
        .unwrap();
    repository.mark_ready(resource_id, 11).unwrap();
    let fingerprint = [7; 32];
    let input = ReserveResourceDelete {
        account_id: account,
        resource_id,
        idempotency_key: "delete-resource",
        fingerprint_key_id: storage.crypto().fingerprint_key_id(),
        request_fingerprint: &fingerprint,
        now_ms: 12,
        expires_at_ms: 100,
    };
    assert!(matches!(
        repository.reserve_delete(&input).unwrap(),
        ResourceDeleteReservation::Reserved(record) if record.id == resource_id
    ));
    assert!(matches!(
        repository.reserve_delete(&input).unwrap(),
        ResourceDeleteReservation::Continue(record) if record.id == resource_id
    ));
    assert_eq!(
        repository
            .reserve_delete(&ReserveResourceDelete {
                request_fingerprint: &[8; 32],
                ..input.clone()
            })
            .unwrap_err()
            .code(),
        ErrorCode::IdempotencyConflict
    );
    repository
        .complete_delete(
            account,
            input.idempotency_key,
            &fingerprint,
            resource_id,
            b"deleted",
        )
        .unwrap();
    assert!(matches!(
        repository.reserve_delete(&input).unwrap(),
        ResourceDeleteReservation::Complete(body) if body == b"deleted"
    ));
    assert_eq!(
        repository
            .complete_delete(
                account,
                input.idempotency_key,
                &fingerprint,
                resource_id,
                b"again",
            )
            .unwrap_err()
            .code(),
        ErrorCode::IdempotencyConflict
    );
    assert_eq!(
        repository
            .reserve_delete(&ReserveResourceDelete {
                idempotency_key: "",
                ..input.clone()
            })
            .unwrap_err()
            .code(),
        ErrorCode::IdempotencyConflict
    );
    assert_eq!(
        repository
            .reserve_delete(&ReserveResourceDelete {
                idempotency_key: "expired-delete",
                expires_at_ms: input.now_ms,
                ..input
            })
            .unwrap_err()
            .code(),
        ErrorCode::ResourceInvariantViolation
    );
}
