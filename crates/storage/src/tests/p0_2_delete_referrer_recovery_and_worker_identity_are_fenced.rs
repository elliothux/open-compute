use super::*;

#[test]
fn p0_2_delete_referrer_recovery_and_worker_identity_are_fenced() {
    let (_tmp, root) = unique_root();
    let config = storage_config(&root);
    let storage = PlatformStorage::bootstrap(&config, &SystemClock).unwrap();
    let account = storage.identity().default_account_id;
    let repo = WorkerRepository::new(storage.db());
    let request = open_compute_core::RequestId::generate();
    let (worker, route) = repo
        .create_worker(account, "delete-gate", request, 1, 1_000_000)
        .unwrap();
    let a = insert_ready(&repo, account, worker.id, [9; 32], request, 10);
    repo.promote(account, worker.id, a, None, request, 11)
        .unwrap();
    let b = insert_ready(&repo, account, worker.id, [9; 32], request, 12);

    repo.add_version_referrer(b, "control_idempotency", "safe-ref", 13)
        .unwrap();
    assert_eq!(repo.version_referrers(b).unwrap().len(), 1);
    assert_eq!(
        repo.begin_version_delete(account, worker.id, b)
            .unwrap_err()
            .code(),
        ErrorCode::VersionReferenced
    );
    repo.remove_version_referrer(b, "control_idempotency", "safe-ref")
        .unwrap();
    repo.begin_version_delete(account, worker.id, b).unwrap();
    assert_eq!(repo.deleting_versions().unwrap(), vec![b]);
    drop(storage); // crash boundary: deleting is committed, finalization is retryable.

    let storage = PlatformStorage::bootstrap(&config, &SystemClock).unwrap();
    let repo = WorkerRepository::new(storage.db());
    assert_eq!(repo.deleting_versions().unwrap(), vec![b]);
    assert_eq!(repo.recover_deleting_versions(request, 20, 64).unwrap(), 1);
    assert!(repo.deleting_versions().unwrap().is_empty());
    assert_eq!(
        repo.get_version(account, worker.id, b).unwrap().state,
        VersionState::Tombstoned
    );
    let refs = repo.referenced_artifacts().unwrap();
    assert_eq!(refs, vec![([9; 32], 100)]);
    assert_eq!(
        repo.begin_version_delete(account, worker.id, a)
            .unwrap_err()
            .code(),
        ErrorCode::VersionActive
    );

    let old = insert_ready(&repo, account, worker.id, [7; 32], request, 40);
    let newest = insert_ready(&repo, account, worker.id, [8; 32], request, 41);
    let candidates = repo.retention_candidates(1_000, 1, 1, 1, 64).unwrap();
    assert!(
        candidates
            .iter()
            .any(|candidate| candidate.version_id == old)
    );
    assert!(
        !candidates
            .iter()
            .any(|candidate| candidate.version_id == newest)
    );

    let expected = repo
        .list_versions(account, worker.id)
        .unwrap()
        .into_iter()
        .filter(|version| version.deleted_at_ms.is_none())
        .map(|version| version.id)
        .collect::<Vec<_>>();
    repo.delete_worker(account, worker.id, &expected, request, 30)
        .unwrap();
    assert_eq!(
        repo.resolve_route(None, &format!("{}x", route.path_prefix))
            .unwrap_err()
            .code(),
        ErrorCode::RouteNotFound
    );
    let (replacement, replacement_route) = repo
        .create_worker(account, "delete-gate", request, 31, 1_000_000)
        .unwrap();
    assert_ne!(replacement.id, worker.id);
    assert_ne!(replacement.do_storage_id, worker.do_storage_id);
    assert_ne!(replacement_route.id, route.id);
}
