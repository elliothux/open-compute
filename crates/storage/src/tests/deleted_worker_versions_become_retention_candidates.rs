use super::*;

#[test]
fn deleted_worker_versions_become_retention_candidates() {
    let (_tmp, root) = unique_root();
    let storage = PlatformStorage::bootstrap(&storage_config(&root), &SystemClock).unwrap();
    let account = storage.identity().instance_id;
    let repo = WorkerRepository::new(storage.db());
    let request = open_compute_core::RequestId::generate();
    let (worker, _) = repo
        .create_worker(account, "reclaim", request, 1, 1_000_000)
        .unwrap();
    let only = insert_ready(&repo, account, worker.id, [4; 32], request, 10);
    repo.promote(account, worker.id, only, None, request, 11)
        .unwrap();

    let live = repo.retention_candidates(1_000, 0, 10, 10, 64).unwrap();
    assert!(live.iter().all(|candidate| candidate.version_id != only));

    repo.delete_worker(account, worker.id, &[only], request, 20)
        .unwrap();
    let deleted = repo.retention_candidates(1_000, 0, 10, 10, 64).unwrap();
    assert!(deleted.iter().any(|candidate| candidate.version_id == only));

    repo.add_version_referrer(only, "control_idempotency", "hold", 21)
        .unwrap();
    let held = repo.retention_candidates(1_000, 0, 10, 10, 64).unwrap();
    assert!(held.iter().all(|candidate| candidate.version_id != only));
    repo.remove_version_referrer(only, "control_idempotency", "hold")
        .unwrap();

    repo.begin_version_delete(account, worker.id, only).unwrap();
    repo.finalize_version_delete(account, worker.id, only, request, 22)
        .unwrap();
    assert!(
        !repo
            .referenced_artifacts()
            .unwrap()
            .iter()
            .any(|(digest, _)| *digest == [4; 32])
    );
}
