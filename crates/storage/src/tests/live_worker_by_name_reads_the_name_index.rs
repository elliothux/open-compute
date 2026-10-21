use super::*;

#[test]
fn live_worker_by_name_reads_the_name_index() {
    let (_tmp, root) = unique_root();
    let storage = PlatformStorage::bootstrap(&storage_config(&root), &SystemClock).unwrap();
    let account = storage.identity().instance_id;
    let repo = WorkerRepository::new(storage.db());
    let request = open_compute_core::RequestId::generate();
    let (alpha, _) = repo
        .create_worker(account, "alpha", request, 1, 1_000_000)
        .unwrap();
    let (beta, _) = repo
        .create_worker(account, "beta", request, 2, 1_000_000)
        .unwrap();

    assert_eq!(
        repo.live_worker_by_name(account, "beta").unwrap().id,
        beta.id
    );
    assert_eq!(
        repo.live_worker_by_name(account, "missing")
            .unwrap_err()
            .code(),
        ErrorCode::WorkerNotFound
    );

    repo.delete_worker(account, alpha.id, &[], request, 3)
        .unwrap();
    assert_eq!(
        repo.live_worker_by_name(account, "alpha")
            .unwrap_err()
            .code(),
        ErrorCode::WorkerNotFound
    );
    assert_eq!(
        repo.live_worker_by_name(account, "beta").unwrap().id,
        beta.id
    );
}
