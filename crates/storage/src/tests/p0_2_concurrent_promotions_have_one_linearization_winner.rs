use super::*;

#[test]
fn p0_2_concurrent_promotions_have_one_linearization_winner() {
    let (_tmp, root) = unique_root();
    let storage = PlatformStorage::bootstrap(&storage_config(&root), &SystemClock).unwrap();
    let account = storage.identity().default_account_id;
    let repo = WorkerRepository::new(storage.db());
    let request = open_compute_core::RequestId::generate();
    let (worker, _) = repo
        .create_worker(account, "promotion-race", request, 1, 1_000_000)
        .unwrap();
    let a = insert_ready(&repo, account, worker.id, [1; 32], request, 10);
    let b = insert_ready(&repo, account, worker.id, [2; 32], request, 11);
    let c = insert_ready(&repo, account, worker.id, [3; 32], request, 12);
    let active = repo
        .promote(account, worker.id, a, None, request, 13)
        .unwrap();
    let generation = active.route_generation;

    let (left, right) = thread::scope(|scope| {
        let left = scope.spawn(|| {
            repo.promote_checked(
                account,
                worker.id,
                b,
                Some(a),
                Some(generation),
                request,
                14,
            )
        });
        let right = scope.spawn(|| {
            repo.promote_checked(
                account,
                worker.id,
                c,
                Some(a),
                Some(generation),
                request,
                14,
            )
        });
        (left.join().unwrap(), right.join().unwrap())
    });
    assert_ne!(left.is_ok(), right.is_ok());
    let loser = left.err().or_else(|| right.err()).unwrap();
    assert_eq!(loser.code(), ErrorCode::IdempotencyConflict);
    let current = repo.get_worker(account, worker.id).unwrap();
    assert!(matches!(current.active_version_id, Some(id) if id == b || id == c));
    assert_eq!(current.route_generation, generation + 1);
}
