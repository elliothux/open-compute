use super::*;

#[test]
fn worker_repository_rejects_invalid_state_and_ownership_operations() {
    let (_tmp, root) = unique_root();
    let storage = PlatformStorage::bootstrap(&storage_config(&root), &SystemClock).unwrap();
    let repo = WorkerRepository::new(storage.db());
    let account = storage.identity().default_account_id;
    let request = open_compute_core::RequestId::generate();

    assert_eq!(
        repo.create_worker(
            AccountId::generate(),
            "missing-account",
            request,
            1,
            1_000_000
        )
        .unwrap_err()
        .code(),
        ErrorCode::AccountNotFound
    );

    let (worker, _) = repo
        .create_worker(account, "state-matrix", request, 2, 1_000_000)
        .unwrap();
    let ready = insert_ready(&repo, account, worker.id, [3; 32], request, 10);
    assert_eq!(
        repo.mark_rejected(ready, VersionState::Ready, ErrorCode::BundleInvalid, 12)
            .unwrap_err()
            .code(),
        ErrorCode::VersionInvariantViolation
    );
    assert_eq!(
        repo.begin_validation(VersionId::generate())
            .unwrap_err()
            .code(),
        ErrorCode::VersionNotReady
    );
    assert_eq!(
        repo.promote(account, worker.id, VersionId::generate(), None, request, 13,)
            .unwrap_err()
            .code(),
        ErrorCode::VersionNotFound
    );

    let staging = VersionId::generate();
    repo.insert_staging_version(
        &NewVersion {
            id: staging,
            account_id: account,
            worker_id: worker.id,
            content_kind: crate::VersionContentKind::Worker,
            artifact_sha256: Some([4; 32]),
            artifact_size: Some(100),
            artifact_schema_version: Some(1),
            main_module: Some("index.js".to_owned()),
            worker_code_sha256: [4; 32],
            compatibility_date: "2026-09-08".into(),
            compatibility_flags: Vec::new(),
            vars: BTreeMap::new(),
            secrets: BTreeMap::new(),
            request_id: request,
            now_ms: 14,
        },
        &crate::NewVersionProducts::default(),
        1_000_000,
    )
    .unwrap();
    assert_eq!(
        repo.promote(account, worker.id, staging, None, request, 15)
            .unwrap_err()
            .code(),
        ErrorCode::VersionNotReady
    );
    let foreign_account = AccountId::generate();
    storage
        .db()
        .with_immediate(|transaction| {
            transaction
                .execute(
                    "INSERT INTO accounts (id, name, created_at_ms, deleted_at_ms)
                     VALUES (?1, ?2, 1, NULL)",
                    rusqlite::params![
                        foreign_account.to_string(),
                        format!("foreign-{foreign_account}")
                    ],
                )
                .map_err(|_| {
                    open_compute_core::PlatformError::new(
                        ErrorCode::Internal,
                        "test account insert",
                    )
                })?;
            Ok(())
        })
        .unwrap();
    let (foreign_worker, _) = repo
        .create_worker(foreign_account, "foreign", request, 16, 1_000_000)
        .unwrap();
    let foreign_ready = insert_ready(
        &repo,
        foreign_account,
        foreign_worker.id,
        [5; 32],
        request,
        17,
    );
    assert_eq!(
        repo.promote(account, worker.id, foreign_ready, None, request, 19)
            .unwrap_err()
            .code(),
        ErrorCode::VersionNotFound
    );
    assert_eq!(
        repo.promote(
            AccountId::generate(),
            worker.id,
            foreign_ready,
            None,
            request,
            19,
        )
        .unwrap_err()
        .code(),
        ErrorCode::WorkerNotFound
    );
    assert_eq!(
        repo.add_version_referrer(staging, "control_idempotency", "ref", 16)
            .unwrap_err()
            .code(),
        ErrorCode::VersionNotReady
    );
    assert_eq!(
        repo.add_version_referrer(VersionId::generate(), "control_idempotency", "ref", 16,)
            .unwrap_err()
            .code(),
        ErrorCode::VersionNotReady
    );

    let fingerprint = [9; 32];
    assert_eq!(
        repo.complete_idempotency(account, "scope", "missing", &fingerprint, b"{}")
            .unwrap_err()
            .code(),
        ErrorCode::IdempotencyConflict
    );
    assert_eq!(
        repo.complete_idempotency_with_version_ref(
            account,
            "scope",
            "missing",
            &fingerprint,
            b"{}",
            ready,
            "wrong-ref",
            17,
        )
        .unwrap_err()
        .code(),
        ErrorCode::VersionInvariantViolation
    );
    let expected_ref = crate::workers::idempotency_ref_id(account, "scope", "missing");
    assert_eq!(
        repo.complete_idempotency_with_version_ref(
            account,
            "scope",
            "missing",
            &fingerprint,
            b"{}",
            ready,
            &expected_ref,
            17,
        )
        .unwrap_err()
        .code(),
        ErrorCode::IdempotencyConflict
    );
    assert_eq!(
        repo.fail_idempotency(account, "scope", "missing", &fingerprint, b"{}")
            .unwrap_err()
            .code(),
        ErrorCode::IdempotencyConflict
    );

    assert_eq!(
        repo.begin_version_delete(account, worker.id, VersionId::generate())
            .unwrap_err()
            .code(),
        ErrorCode::VersionNotFound
    );
    repo.begin_version_delete(account, worker.id, ready)
        .unwrap();
    repo.begin_version_delete(account, worker.id, ready)
        .unwrap();
    assert_eq!(
        repo.finalize_version_delete(account, worker.id, staging, request, 18)
            .unwrap_err()
            .code(),
        ErrorCode::VersionNotFound
    );
}

#[test]
fn worker_repository_rejects_invalid_routes_retention_and_deletion() {
    let (_tmp, root) = unique_root();
    let storage = PlatformStorage::bootstrap(&storage_config(&root), &SystemClock).unwrap();
    let repo = WorkerRepository::new(storage.db());
    let account = storage.identity().default_account_id;
    let request = open_compute_core::RequestId::generate();
    let (worker, _) = repo
        .create_worker(account, "state-matrix", request, 2, 1_000_000)
        .unwrap();
    let ready = insert_ready(&repo, account, worker.id, [3; 32], request, 10);
    repo.begin_version_delete(account, worker.id, ready)
        .unwrap();
    let staging = VersionId::generate();
    repo.insert_staging_version(
        &NewVersion {
            id: staging,
            account_id: account,
            worker_id: worker.id,
            content_kind: crate::VersionContentKind::Worker,
            artifact_sha256: Some([4; 32]),
            artifact_size: Some(100),
            artifact_schema_version: Some(1),
            main_module: Some("index.js".to_owned()),
            worker_code_sha256: [4; 32],
            compatibility_date: "2026-09-08".into(),
            compatibility_flags: Vec::new(),
            vars: BTreeMap::new(),
            secrets: BTreeMap::new(),
            request_id: request,
            now_ms: 14,
        },
        &crate::NewVersionProducts::default(),
        1_000_000,
    )
    .unwrap();
    assert_eq!(
        repo.version_snapshot(account, worker.id, staging, false)
            .unwrap_err()
            .code(),
        ErrorCode::VersionNotReady
    );
    let promotable = insert_ready(&repo, account, worker.id, [10; 32], request, 19);
    assert_eq!(
        repo.promote_checked(
            account,
            worker.id,
            promotable,
            Some(VersionId::generate()),
            None,
            request,
            19,
        )
        .unwrap_err()
        .code(),
        ErrorCode::IdempotencyConflict
    );
    repo.promote(account, worker.id, promotable, None, request, 20)
        .unwrap();
    assert_eq!(
        repo.create_exact_route(
            account,
            worker.id,
            "conflict.example",
            "/",
            None,
            Some(VersionId::generate()),
            request,
            21,
            1_000_000,
        )
        .unwrap_err()
        .code(),
        ErrorCode::IdempotencyConflict
    );
    let route = repo
        .create_exact_route(
            account,
            worker.id,
            "conflict.example",
            "/",
            None,
            Some(promotable),
            request,
            22,
            1_000_000,
        )
        .unwrap();
    assert_eq!(
        repo.create_exact_route(
            account,
            worker.id,
            "conflict.example",
            "/",
            None,
            Some(promotable),
            request,
            23,
            1_000_000,
        )
        .unwrap_err()
        .code(),
        ErrorCode::RouteConflict
    );
    assert_eq!(
        repo.delete_route(account, worker.id, "missing-route", request, 24)
            .unwrap_err()
            .code(),
        ErrorCode::RouteNotFound
    );
    repo.delete_route(account, worker.id, &route.id, request, 25)
        .unwrap();

    let invalid_state_fingerprint = [11; 32];
    repo.reserve_idempotency(
        account,
        "invalid-state",
        "key",
        "fingerprint-key",
        &invalid_state_fingerprint,
        26,
        100,
    )
    .unwrap();
    storage
        .db()
        .with_read(|conn| {
            conn.execute(
                "UPDATE control_idempotency SET state = 'complete', response_json = NULL
                 WHERE account_id = ?1 AND scope = 'invalid-state' AND idempotency_key = 'key'",
                [account.to_string()],
            )
            .map_err(|_| {
                open_compute_core::PlatformError::new(ErrorCode::Internal, "test update failed")
            })?;
            Ok(())
        })
        .unwrap();
    assert_eq!(
        repo.reserve_idempotency(
            account,
            "invalid-state",
            "key",
            "fingerprint-key",
            &invalid_state_fingerprint,
            27,
            100,
        )
        .unwrap_err()
        .code(),
        ErrorCode::Internal
    );

    let referenced = insert_ready(&repo, account, worker.id, [12; 32], request, 30);
    repo.begin_version_delete(account, worker.id, referenced)
        .unwrap();
    storage
        .db()
        .with_read(|conn| {
            conn.execute(
                "INSERT INTO version_referrers
                 (version_id, kind, ref_id, created_at_ms) VALUES (?1, 'test', 'late', 31)",
                [referenced.to_string()],
            )
            .map_err(|_| {
                open_compute_core::PlatformError::new(ErrorCode::Internal, "test insert failed")
            })?;
            Ok(())
        })
        .unwrap();
    assert_eq!(
        repo.finalize_version_delete(account, worker.id, referenced, request, 32)
            .unwrap_err()
            .code(),
        ErrorCode::VersionReferenced
    );

    let tombstone = insert_ready(&repo, account, worker.id, [13; 32], request, 33);
    repo.tombstone_version(account, worker.id, tombstone, request, 34)
        .unwrap();

    for args in [(0, 1, 1), (1, 0, 1), (1, 1, 0), (1, 1, 10_001)] {
        assert_eq!(
            repo.retention_candidates(40, 0, args.0, args.1, args.2)
                .unwrap_err()
                .code(),
            ErrorCode::LimitInvalid
        );
    }

    let expected = repo
        .list_versions(account, worker.id)
        .unwrap()
        .into_iter()
        .filter(|version| version.deleted_at_ms.is_none())
        .map(|version| version.id)
        .collect::<Vec<_>>();
    repo.delete_worker(account, worker.id, &expected, request, 41)
        .unwrap();
    assert_eq!(
        repo.version_snapshot(account, worker.id, ready, false)
            .unwrap_err()
            .code(),
        ErrorCode::WorkerDeleted
    );
    assert_eq!(
        repo.delete_worker(account, worker.id, &[], request, 42)
            .unwrap_err()
            .code(),
        ErrorCode::WorkerDeleted
    );

    for invalid in [0, 10_001] {
        assert_eq!(
            repo.recover_deleting_versions(request, 19, invalid)
                .unwrap_err()
                .code(),
            ErrorCode::LimitInvalid
        );
        assert_eq!(
            repo.prune_expired_idempotency(19, invalid)
                .unwrap_err()
                .code(),
            ErrorCode::LimitInvalid
        );
    }
}
