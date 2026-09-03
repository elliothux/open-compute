use super::*;
use crate::{
    D1DatabaseRepository, D1Paths, PlatformStorage, ReserveResourceCreate,
    ResourceCreateReservation, ResourceRepository,
};
use open_compute_core::config::StorageConfig;
use open_compute_core::{BindingKind, RequestId, SystemClock};

const QUOTA: u64 = 256 * 1024 * 1024;

fn config(root: &std::path::Path) -> StorageConfig {
    StorageConfig {
        data_dir: root.to_path_buf(),
        master_key_file: root.join("keys/master.key"),
        master_key_env: None,
        sqlite_busy_timeout_ms: 5_000,
        free_space_soft_bytes: 1_073_741_824,
        free_space_hard_bytes: 268_435_456,
    }
}

fn fixture() -> (
    tempfile::TempDir,
    StorageConfig,
    PlatformStorage,
    AccountId,
    ResourceId,
) {
    let temp = tempfile::tempdir().unwrap();
    let config = config(&temp.path().join("data"));
    let storage = PlatformStorage::bootstrap(&config, &SystemClock).unwrap();
    let account = storage.identity().default_account_id;
    let fingerprint = storage.crypto().fingerprint_request(b"d1-history-database");
    let ResourceCreateReservation::Reserved(resource) = ResourceRepository::new(storage.db())
        .reserve_create(
            &ReserveResourceCreate {
                account_id: account,
                kind: BindingKind::D1Database,
                name: "history-db",
                idempotency_key: "history-db",
                fingerprint_key_id: storage.crypto().fingerprint_key_id(),
                request_fingerprint: &fingerprint,
                resource_id: ResourceId::generate(),
                driver_schema_version: 1,
                request_id: RequestId::generate(),
                now_ms: 1,
                expires_at_ms: 1_000,
            },
            1_000_000,
        )
        .unwrap()
    else {
        panic!("first reservation must create a resource");
    };
    let resource_id = resource.id;
    let key = D1Paths::storage_key(account, resource_id);
    D1DatabaseRepository::new(storage.db())
        .ensure_database(&resource, &key, 1, QUOTA)
        .unwrap();
    (temp, config, storage, account, resource_id)
}

fn snapshot_key(resource: ResourceId, version: u64) -> String {
    format!("d1/history/{resource}/{version}/data.sqlite")
}

#[test]
fn completed_history_is_sparse_replay_safe_and_timestamp_resolved() {
    let (_temp, _config, storage, account, resource) = fixture();
    let history = D1SnapshotRepository::new(storage.db());
    let zero = history
        .record_completed_snapshot(
            account,
            resource,
            0,
            &snapshot_key(resource, 0),
            &[1; 32],
            100,
            10,
        )
        .unwrap();
    assert_eq!(zero.session_version, 0);
    assert_eq!(
        history
            .record_completed_snapshot(
                account,
                resource,
                0,
                &snapshot_key(resource, 0),
                &[1; 32],
                100,
                99,
            )
            .unwrap(),
        zero
    );
    assert_eq!(
        history
            .record_completed_snapshot(
                account,
                resource,
                0,
                &snapshot_key(resource, 0),
                &[2; 32],
                100,
                99,
            )
            .unwrap_err()
            .code(),
        ErrorCode::IdempotencyConflict
    );
    let two = history
        .record_completed_snapshot(
            account,
            resource,
            2,
            &snapshot_key(resource, 2),
            &[2; 32],
            101,
            20,
        )
        .unwrap();
    assert_eq!(
        history
            .record_completed_snapshot(
                account,
                resource,
                1,
                &snapshot_key(resource, 1),
                &[3; 32],
                100,
                21,
            )
            .unwrap_err()
            .code(),
        ErrorCode::ResourceInvariantViolation
    );
    assert_eq!(
        history.latest_snapshot(account, resource).unwrap(),
        Some(two.clone())
    );
    assert_eq!(
        history.snapshot_at_or_before(account, resource, 9).unwrap(),
        None
    );
    assert_eq!(
        history
            .snapshot_at_or_before(account, resource, 10)
            .unwrap(),
        Some(zero)
    );
    assert_eq!(
        history
            .snapshot_at_or_before(account, resource, 20)
            .unwrap(),
        Some(two)
    );
    assert_eq!(
        history
            .latest_snapshot(AccountId::generate(), resource)
            .unwrap_err()
            .code(),
        ErrorCode::ResourceNotFound
    );
}

#[test]
fn transfer_session_survives_restart_and_enforces_capability_and_ingest_fences() {
    let (_temp, config, storage, account, resource) = fixture();
    let history = D1SnapshotRepository::new(storage.db());
    history
        .record_completed_snapshot(
            account,
            resource,
            0,
            &snapshot_key(resource, 0),
            &[1; 32],
            100,
            10,
        )
        .unwrap();

    let import_id = uuid::Uuid::now_v7().hyphenated().to_string();
    let etag = [3; 16];
    let token = [4; 32];
    let new_import = NewD1Transfer {
        id: &import_id,
        account_id: account,
        resource_id: resource,
        kind: D1TransferKind::Import,
        at_session_version: 0,
        filename: "import.sql",
        etag_md5: Some(&etag),
        token_fingerprint: &token,
        token_action: D1TransferAction::Upload,
        token_expires_at_ms: 100,
        now_ms: 20,
    };
    let uploading = history.create_transfer(&new_import).unwrap();
    assert_eq!(uploading.state, D1TransferState::Uploading);
    assert_eq!(history.create_transfer(&new_import).unwrap(), uploading);
    assert_eq!(
        history
            .transfer_by_filename(account, resource, D1TransferKind::Import, "import.sql")
            .unwrap(),
        Some(uploading.clone())
    );
    let conflicting = NewD1Transfer {
        id: &uuid::Uuid::now_v7().hyphenated().to_string(),
        ..new_import
    };
    assert_eq!(
        history.create_transfer(&conflicting).unwrap_err().code(),
        ErrorCode::IdempotencyConflict
    );
    assert_eq!(
        history
            .authorize_transfer_token(
                account,
                resource,
                &import_id,
                D1TransferAction::Download,
                &token,
                21,
            )
            .unwrap_err()
            .code(),
        ErrorCode::ResourceNotFound
    );
    assert_eq!(
        history
            .complete_upload(
                account,
                &import_id,
                "d1/transfers/import.sql",
                &[9; 16],
                &[5; 32],
                120,
                30,
            )
            .unwrap_err()
            .code(),
        ErrorCode::ResourceInvariantViolation
    );
    let uploaded = history
        .complete_upload(
            account,
            &import_id,
            "d1/transfers/import.sql",
            &etag,
            &[5; 32],
            120,
            30,
        )
        .unwrap();
    drop(storage);

    let storage = PlatformStorage::bootstrap(&config, &SystemClock).unwrap();
    let history = D1SnapshotRepository::new(storage.db());
    assert_eq!(
        history
            .authorize_transfer_token(
                account,
                resource,
                &import_id,
                D1TransferAction::Upload,
                &token,
                34,
            )
            .unwrap()
            .state,
        D1TransferState::Uploaded
    );
    assert_eq!(
        history
            .complete_upload(
                account,
                &import_id,
                "d1/transfers/import.sql",
                &etag,
                &[5; 32],
                120,
                35,
            )
            .unwrap(),
        uploaded
    );
    let ingesting = history
        .begin_ingest(account, &import_id, 3, 1.25, 1, 2, 4096, 40)
        .unwrap();
    assert_eq!(ingesting.state, D1TransferState::Ingesting);
    assert_eq!(ingesting.num_queries, Some(3));
    assert_eq!(
        history
            .begin_ingest(account, &import_id, 3, 9.0, 1, 2, 4096, 45)
            .unwrap(),
        ingesting
    );
    assert_eq!(
        history
            .begin_ingest(account, &import_id, 4, 1.25, 1, 2, 4096, 45)
            .unwrap_err()
            .code(),
        ErrorCode::IdempotencyConflict
    );
    assert_eq!(
        history.transfer(account, &import_id).unwrap().state,
        D1TransferState::Ingesting
    );
    assert_eq!(
        history
            .record_completed_snapshot(
                account,
                resource,
                0,
                &snapshot_key(resource, 0),
                &[1; 32],
                100,
                41,
            )
            .unwrap()
            .created_at_ms,
        10
    );
    history
        .record_completed_snapshot(
            account,
            resource,
            1,
            &snapshot_key(resource, 1),
            &[6; 32],
            130,
            50,
        )
        .unwrap();
    let complete = history
        .complete_import(account, &import_id, 1, 3, 60)
        .unwrap();
    assert_eq!(complete.state, D1TransferState::Complete);
    assert_eq!(complete.result_session_version, Some(1));
    drop(storage);

    let storage = PlatformStorage::bootstrap(&config, &SystemClock).unwrap();
    let history = D1SnapshotRepository::new(storage.db());
    assert_eq!(
        history
            .complete_import(account, &import_id, 1, 3, 70)
            .unwrap(),
        complete
    );
    assert!(
        history
            .active_transfer(account, resource)
            .unwrap()
            .is_none()
    );
}

#[test]
fn uploaded_failure_retains_evidence_and_restore_intent_reconciles_after_restart() {
    let (_temp, config, storage, account, resource) = fixture();
    let history = D1SnapshotRepository::new(storage.db());
    history
        .record_completed_snapshot(
            account,
            resource,
            0,
            &snapshot_key(resource, 0),
            &[1; 32],
            100,
            10,
        )
        .unwrap();
    let import_id = uuid::Uuid::now_v7().hyphenated().to_string();
    let etag = [3; 16];
    history
        .create_transfer(&NewD1Transfer {
            id: &import_id,
            account_id: account,
            resource_id: resource,
            kind: D1TransferKind::Import,
            at_session_version: 0,
            filename: "failed.sql",
            etag_md5: Some(&etag),
            token_fingerprint: &[4; 32],
            token_action: D1TransferAction::Upload,
            token_expires_at_ms: 100,
            now_ms: 20,
        })
        .unwrap();
    history
        .complete_upload(
            account,
            &import_id,
            "d1/transfers/failed.sql",
            &etag,
            &[5; 32],
            120,
            30,
        )
        .unwrap();
    let failed = history
        .fail_transfer(account, &import_id, ErrorCode::D1SqlInvalid, 40)
        .unwrap();
    assert_eq!(failed.state, D1TransferState::Failed);
    assert_eq!(failed.file_key.as_deref(), Some("d1/transfers/failed.sql"));
    assert_eq!(failed.sha256, Some([5; 32]));
    assert_eq!(failed.size_bytes, Some(120));

    drop(storage);

    let storage = PlatformStorage::bootstrap(&config, &SystemClock).unwrap();
    let history = D1SnapshotRepository::new(storage.db());
    assert_eq!(
        history
            .fail_transfer(account, &import_id, ErrorCode::D1SqlInvalid, 45)
            .unwrap(),
        failed
    );
    let restore_id = uuid::Uuid::now_v7().hyphenated().to_string();
    let fingerprint = [8; 32];
    let intent = history
        .prepare_restore(account, resource, &restore_id, 0, 0, &fingerprint, 50)
        .unwrap();
    assert_eq!(intent.result_session_version, 1);
    drop(storage);

    let storage = PlatformStorage::bootstrap(&config, &SystemClock).unwrap();
    let history = D1SnapshotRepository::new(storage.db());
    assert_eq!(
        history
            .prepare_restore(account, resource, &restore_id, 0, 0, &fingerprint, 55)
            .unwrap(),
        intent
    );
    assert_eq!(
        history
            .prepare_restore(
                account,
                resource,
                &uuid::Uuid::now_v7().hyphenated().to_string(),
                0,
                0,
                &fingerprint,
                56,
            )
            .unwrap_err()
            .code(),
        ErrorCode::IdempotencyConflict
    );
    assert_eq!(
        history.pending_restore(account, resource).unwrap(),
        Some(intent)
    );
    assert_eq!(
        history
            .complete_restore(account, resource, &restore_id)
            .unwrap_err()
            .code(),
        ErrorCode::ResourceNotFound
    );
    history
        .record_completed_snapshot(
            account,
            resource,
            1,
            &snapshot_key(resource, 1),
            &[9; 32],
            110,
            60,
        )
        .unwrap();
    history
        .complete_restore(account, resource, &restore_id)
        .unwrap();
    assert!(
        history
            .pending_restore(account, resource)
            .unwrap()
            .is_none()
    );
}

#[test]
fn terminal_transfer_replays_keep_original_times_after_restart() {
    let (_temp, config, storage, account, resource) = fixture();
    let history = D1SnapshotRepository::new(storage.db());
    history
        .record_completed_snapshot(
            account,
            resource,
            0,
            &snapshot_key(resource, 0),
            &[1; 32],
            100,
            10,
        )
        .unwrap();

    let export_id = uuid::Uuid::now_v7().hyphenated().to_string();
    history
        .create_transfer(&NewD1Transfer {
            id: &export_id,
            account_id: account,
            resource_id: resource,
            kind: D1TransferKind::Export,
            at_session_version: 0,
            filename: "export-one.sql",
            etag_md5: None,
            token_fingerprint: &[2; 32],
            token_action: D1TransferAction::Download,
            token_expires_at_ms: 100,
            now_ms: 20,
        })
        .unwrap();
    let complete = history
        .complete_export(
            account,
            &export_id,
            "d1/transfers/export-one.sql",
            &[3; 32],
            200,
            30,
        )
        .unwrap();

    let expired_id = uuid::Uuid::now_v7().hyphenated().to_string();
    history
        .create_transfer(&NewD1Transfer {
            id: &expired_id,
            account_id: account,
            resource_id: resource,
            kind: D1TransferKind::Import,
            at_session_version: 0,
            filename: "expired.sql",
            etag_md5: Some(&[4; 16]),
            token_fingerprint: &[5; 32],
            token_action: D1TransferAction::Upload,
            token_expires_at_ms: 50,
            now_ms: 40,
        })
        .unwrap();
    let expired = history.expire_transfer(account, &expired_id, 50).unwrap();
    drop(storage);

    let storage = PlatformStorage::bootstrap(&config, &SystemClock).unwrap();
    let history = D1SnapshotRepository::new(storage.db());
    assert_eq!(
        history
            .complete_export(
                account,
                &export_id,
                "d1/transfers/export-one.sql",
                &[3; 32],
                200,
                60,
            )
            .unwrap(),
        complete
    );
    assert_eq!(
        history.expire_transfer(account, &expired_id, 70).unwrap(),
        expired
    );
}

#[test]
fn expired_terminal_transfer_gc_releases_completed_history_capacity() {
    let (_temp, _config, storage, account, resource) = fixture();
    let history = D1SnapshotRepository::new(storage.db());
    history
        .record_completed_snapshot(
            account,
            resource,
            0,
            &snapshot_key(resource, 0),
            &[1; 32],
            100,
            10,
        )
        .unwrap();
    let export_id = uuid::Uuid::now_v7().hyphenated().to_string();
    history
        .create_transfer(&NewD1Transfer {
            id: &export_id,
            account_id: account,
            resource_id: resource,
            kind: D1TransferKind::Export,
            at_session_version: 0,
            filename: "bounded-export.sql",
            etag_md5: None,
            token_fingerprint: &[2; 32],
            token_action: D1TransferAction::Download,
            token_expires_at_ms: 100,
            now_ms: 20,
        })
        .unwrap();
    history
        .complete_export(
            account,
            &export_id,
            "d1/transfers/bounded-export.sql",
            &[3; 32],
            200,
            30,
        )
        .unwrap();

    assert_eq!(
        history
            .ensure_completed_snapshot_capacity(account, resource, 1, [None, None])
            .unwrap_err()
            .code(),
        ErrorCode::D1DatabaseFull,
    );
    assert_eq!(
        history
            .ensure_transfer_file_capacity(account, resource, 1, 99)
            .unwrap_err()
            .code(),
        ErrorCode::D1DatabaseFull,
    );
    assert!(
        history
            .prune_expired_terminal_transfers(account, resource, 99)
            .unwrap()
            .is_empty()
    );
    let removed = history
        .prune_expired_terminal_transfers(account, resource, 100)
        .unwrap();
    assert_eq!(removed.len(), 1);
    assert_eq!(removed[0].id, export_id);
    assert_eq!(
        history.transfer(account, &export_id).unwrap_err().code(),
        ErrorCode::ResourceNotFound,
    );
    history
        .ensure_completed_snapshot_capacity(account, resource, 1, [None, None])
        .unwrap();
    history
        .ensure_transfer_file_capacity(account, resource, 1, 100)
        .unwrap();
}

#[test]
fn sql_guards_reject_snapshot_transfer_and_restore_corruption() {
    let (_temp, _config, storage, account, resource) = fixture();
    let history = D1SnapshotRepository::new(storage.db());
    history
        .record_completed_snapshot(
            account,
            resource,
            0,
            &snapshot_key(resource, 0),
            &[1; 32],
            100,
            10,
        )
        .unwrap();
    let transfer_id = uuid::Uuid::now_v7().hyphenated().to_string();
    history
        .create_transfer(&NewD1Transfer {
            id: &transfer_id,
            account_id: account,
            resource_id: resource,
            kind: D1TransferKind::Import,
            at_session_version: 0,
            filename: "guard.sql",
            etag_md5: Some(&[2; 16]),
            token_fingerprint: &[3; 32],
            token_action: D1TransferAction::Upload,
            token_expires_at_ms: 100,
            now_ms: 20,
        })
        .unwrap();
    storage
        .db()
        .with_immediate(|tx| {
            assert!(
                tx.execute(
                    "UPDATE d1_snapshots SET size_bytes = 101
                     WHERE resource_id = ?1 AND session_version = 0",
                    [resource.to_string()],
                )
                .is_err()
            );
            assert!(
                tx.execute(
                    "UPDATE d1_transfer_sessions SET token_expires_at_ms = 200 WHERE id = ?1",
                    [&transfer_id],
                )
                .is_err()
            );
            assert!(
                tx.execute(
                    "UPDATE d1_transfer_sessions SET state = 'ingesting', updated_at_ms = 21
                     WHERE id = ?1",
                    [&transfer_id],
                )
                .is_err()
            );
            Ok(())
        })
        .unwrap();
    history
        .fail_transfer(account, &transfer_id, ErrorCode::D1SqlInvalid, 30)
        .unwrap();
    storage
        .db()
        .with_immediate(|tx| {
            assert!(
                tx.execute(
                    "UPDATE d1_transfer_sessions SET updated_at_ms = 31 WHERE id = ?1",
                    [&transfer_id],
                )
                .is_err()
            );
            Ok(())
        })
        .unwrap();

    let restore_id = uuid::Uuid::now_v7().hyphenated().to_string();
    history
        .prepare_restore(account, resource, &restore_id, 0, 0, &[4; 32], 40)
        .unwrap();
    storage
        .db()
        .with_immediate(|tx| {
            assert!(
                tx.execute(
                    "UPDATE d1_restore_intents SET result_session_version = 2
                     WHERE resource_id = ?1",
                    [resource.to_string()],
                )
                .is_err()
            );
            Ok(())
        })
        .unwrap();
}
