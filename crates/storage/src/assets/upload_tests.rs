use super::*;
use crate::{PlatformStorage, WorkerRepository};
use open_compute_core::{RequestId, StorageConfig, SystemClock};

fn storage_config(root: &std::path::Path) -> StorageConfig {
    StorageConfig {
        data_dir: root.to_owned(),
        master_key_file: root.join("keys/master.key"),
        master_key_env: None,
        sqlite_busy_timeout_ms: 5_000,
        free_space_soft_bytes: 1_073_741_824,
        free_space_hard_bytes: 268_435_456,
    }
}

fn new_upload<'a>(
    account_id: AccountId,
    worker_id: WorkerId,
    idempotency_key: &'a str,
    fingerprint: [u8; 32],
    objects: &'a [NewVersionUploadObject],
    now_ms: i64,
) -> NewVersionUpload<'a> {
    NewVersionUpload {
        id: VersionUploadId::generate(),
        account_id,
        worker_id,
        idempotency_key,
        input_fingerprint: fingerprint,
        content_kind: VersionContentKind::AssetsOnly,
        bundle: None,
        manifest_sha256: [1; 32],
        manifest_json: b"{}",
        routing_config_json: b"{}",
        objects,
        now_ms,
        expires_at_ms: now_ms + 100,
    }
}

#[test]
fn upload_sessions_are_scoped_idempotent_bounded_and_transactional() {
    let temp = tempfile::tempdir().unwrap();
    let storage =
        PlatformStorage::bootstrap(&storage_config(&temp.path().join("data")), &SystemClock)
            .unwrap();
    let account = storage.identity().default_account_id;
    let worker = WorkerRepository::new(storage.db())
        .create_worker(account, "assets", RequestId::generate(), 1, 100)
        .unwrap()
        .0;
    let objects = vec![
        NewVersionUploadObject {
            sha256: [1; 32],
            kind: VersionObjectKind::AssetManifest,
            size: 2,
        },
        NewVersionUploadObject {
            sha256: [2; 32],
            kind: VersionObjectKind::AssetBlob,
            size: 7,
        },
    ];
    let repo = VersionUploadRepository::new(storage.db());
    let first_input = new_upload(account, worker.id, "same", [3; 32], &objects, 10);
    let first = repo.create_or_get(&first_input, 2, 4).unwrap();
    assert_eq!(first.status, VersionUploadStatus::Open);
    assert_eq!(first.objects.len(), 2);

    let replay_input = new_upload(account, worker.id, "same", [3; 32], &objects, 11);
    assert_eq!(
        repo.create_or_get(&replay_input, 2, 4).unwrap().id,
        first.id
    );
    let conflict_input = new_upload(account, worker.id, "same", [4; 32], &objects, 11);
    assert_eq!(
        repo.create_or_get(&conflict_input, 2, 4)
            .unwrap_err()
            .code(),
        ErrorCode::AssetUploadConflict
    );
    assert_eq!(
        repo.get(AccountId::generate(), worker.id, first.id, 11)
            .unwrap_err()
            .code(),
        ErrorCode::VersionNotFound
    );

    repo.mark_object_verified(account, worker.id, first.id, &[1; 32], 2, 12)
        .unwrap();
    assert_eq!(
        repo.begin_finalize(BeginVersionUploadFinalize {
            account_id: account,
            worker_id: worker.id,
            upload_id: first.id,
            version_id: VersionId::generate(),
            finalize_fingerprint: [9; 32],
            owner_startup_id: storage.data_dir().startup_id(),
            now_ms: 13,
        })
        .unwrap_err()
        .code(),
        ErrorCode::AssetUploadIncomplete
    );
    repo.mark_object_verified(account, worker.id, first.id, &[2; 32], 7, 14)
        .unwrap();
    let version = VersionId::generate();
    let finalizing = repo
        .begin_finalize(BeginVersionUploadFinalize {
            account_id: account,
            worker_id: worker.id,
            upload_id: first.id,
            version_id: version,
            finalize_fingerprint: [9; 32],
            owner_startup_id: storage.data_dir().startup_id(),
            now_ms: 15,
        })
        .unwrap();
    assert_eq!(
        finalizing.disposition,
        VersionUploadFinalizeDisposition::Reserved
    );
    assert_eq!(finalizing.upload.status, VersionUploadStatus::Finalizing);
    assert_eq!(finalizing.upload.version_id, Some(version));
    assert_eq!(
        repo.begin_finalize(BeginVersionUploadFinalize {
            account_id: account,
            worker_id: worker.id,
            upload_id: first.id,
            version_id: version,
            finalize_fingerprint: [8; 32],
            owner_startup_id: storage.data_dir().startup_id(),
            now_ms: 16,
        })
        .unwrap_err()
        .code(),
        ErrorCode::AssetUploadConflict
    );
    assert_eq!(
        repo.abort(account, worker.id, first.id, 17)
            .unwrap_err()
            .code(),
        ErrorCode::AssetUploadConflict
    );
    assert_eq!(
        repo.mark_committed(account, worker.id, first.id, version, br#"{"ok":true}"#, 18)
            .unwrap()
            .status,
        VersionUploadStatus::Committed
    );
    assert_eq!(
        repo.mark_committed(account, worker.id, first.id, version, br#"{"ok":true}"#, 19)
            .unwrap()
            .status,
        VersionUploadStatus::Committed
    );
    let replay = repo
        .begin_finalize(BeginVersionUploadFinalize {
            account_id: account,
            worker_id: worker.id,
            upload_id: first.id,
            version_id: version,
            finalize_fingerprint: [9; 32],
            owner_startup_id: storage.data_dir().startup_id(),
            now_ms: 20,
        })
        .unwrap();
    assert_eq!(
        replay.disposition,
        VersionUploadFinalizeDisposition::Committed
    );
    assert_eq!(
        replay.upload.finalize_response_json.unwrap(),
        br#"{"ok":true}"#
    );

    let failed_input = new_upload(account, worker.id, "failed", [5; 32], &objects, 30);
    let failed = repo.create_or_get(&failed_input, 2, 4).unwrap();
    assert_eq!(
        repo.object_for_upload(account, worker.id, failed.id, &[9; 32], 31)
            .unwrap_err()
            .code(),
        ErrorCode::VersionNotFound
    );
    assert_eq!(
        repo.mark_object_verified(account, worker.id, failed.id, &[2; 32], 8, 31)
            .unwrap_err()
            .code(),
        ErrorCode::AssetUploadConflict
    );
    for (digest, size) in [([1; 32], 2), ([2; 32], 7)] {
        repo.mark_object_verified(account, worker.id, failed.id, &digest, size, 32)
            .unwrap();
    }
    let failed_version = VersionId::generate();
    let failed_begin = BeginVersionUploadFinalize {
        account_id: account,
        worker_id: worker.id,
        upload_id: failed.id,
        version_id: failed_version,
        finalize_fingerprint: [6; 32],
        owner_startup_id: storage.data_dir().startup_id(),
        now_ms: 33,
    };
    repo.begin_finalize(failed_begin).unwrap();
    assert_eq!(
        repo.begin_finalize(BeginVersionUploadFinalize {
            now_ms: 34,
            ..failed_begin
        })
        .unwrap()
        .disposition,
        VersionUploadFinalizeDisposition::Recover
    );
    let terminal = repo
        .mark_finalize_failed(
            account,
            worker.id,
            failed.id,
            failed_version,
            ErrorCode::BundleInvalid,
            35,
        )
        .unwrap();
    assert_eq!(terminal.status, VersionUploadStatus::Committed);
    assert_eq!(
        terminal.finalize_error_code.as_deref(),
        Some(ErrorCode::BundleInvalid.as_str())
    );
    assert_eq!(
        repo.mark_finalize_failed(
            account,
            worker.id,
            failed.id,
            failed_version,
            ErrorCode::Internal,
            36,
        )
        .unwrap_err()
        .code(),
        ErrorCode::AssetUploadConflict
    );

    let aborted_input = new_upload(account, worker.id, "aborted", [7; 32], &objects, 40);
    let aborted = repo.create_or_get(&aborted_input, 2, 4).unwrap();
    assert_eq!(
        repo.abort(account, worker.id, aborted.id, 41)
            .unwrap()
            .status,
        VersionUploadStatus::Aborted
    );
    assert_eq!(
        repo.abort(account, worker.id, aborted.id, 42)
            .unwrap()
            .status,
        VersionUploadStatus::Aborted
    );
}

#[test]
fn upload_session_quota_and_expiration_fail_closed() {
    let temp = tempfile::tempdir().unwrap();
    let storage =
        PlatformStorage::bootstrap(&storage_config(&temp.path().join("data")), &SystemClock)
            .unwrap();
    let account = storage.identity().default_account_id;
    let worker = WorkerRepository::new(storage.db())
        .create_worker(account, "quota", RequestId::generate(), 1, 100)
        .unwrap()
        .0;
    let objects = [NewVersionUploadObject {
        sha256: [1; 32],
        kind: VersionObjectKind::AssetManifest,
        size: 2,
    }];
    let repo = VersionUploadRepository::new(storage.db());
    for (index, key) in ["one", "two"].into_iter().enumerate() {
        repo.create_or_get(
            &new_upload(account, worker.id, key, [index as u8; 32], &objects, 10),
            2,
            4,
        )
        .unwrap();
    }
    assert_eq!(
        repo.create_or_get(
            &new_upload(account, worker.id, "three", [3; 32], &objects, 10),
            2,
            4,
        )
        .unwrap_err()
        .code(),
        ErrorCode::AssetLimitExceeded
    );
    let expiring = new_upload(
        account,
        worker.id,
        "expired-after-quota",
        [4; 32],
        &objects,
        200,
    );
    let created = repo.create_or_get(&expiring, 2, 4).unwrap();
    assert_eq!(
        repo.get(account, worker.id, created.id, 301)
            .unwrap()
            .status,
        VersionUploadStatus::Expired
    );
    assert_eq!(
        repo.object_for_upload(account, worker.id, created.id, &[1; 32], 302)
            .unwrap_err()
            .code(),
        ErrorCode::AssetUploadConflict
    );
}
