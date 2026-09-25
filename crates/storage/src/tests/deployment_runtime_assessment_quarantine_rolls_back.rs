use super::*;

#[test]
fn deployment_runtime_assessment_quarantine_rolls_back() {
    let (_tmp, root) = unique_root();
    let storage = PlatformStorage::bootstrap(&storage_config(&root), &SystemClock).unwrap();
    let account = storage.identity().instance_id;
    let repo = WorkerRepository::new(storage.db());
    let request = open_compute_core::RequestId::generate();
    let (worker, _) = repo
        .create_worker(account, "runtime-rollback", request, 1, 1_000_000)
        .unwrap();
    let first = insert_ready(&repo, account, worker.id, [1; 32], request, 10);
    let second = insert_ready(&repo, account, worker.id, [2; 32], request, 20);
    let first_worker = repo
        .promote(account, worker.id, first, None, request, 30)
        .unwrap();
    let first_deployment = first_worker.active_deployment_id.unwrap();
    let second_worker = repo
        .promote(account, worker.id, second, Some(first), request, 40)
        .unwrap();
    let second_deployment = second_worker.active_deployment_id.unwrap();
    assert!(
        repo.quarantine_active_deployment(
            second_deployment,
            "RUNTIME_UNEXPECTED_EXIT",
            request,
            50,
        )
        .unwrap()
    );
    let rolled_back = repo.get_worker(account, worker.id).unwrap();
    assert_eq!(rolled_back.active_deployment_id, Some(first_deployment));
    assert_eq!(rolled_back.active_version_id, Some(first));
    assert_eq!(
        repo.promote(account, worker.id, second, Some(first), request, 55)
            .unwrap_err()
            .code(),
        ErrorCode::VersionNotReady
    );
    assert!(
        !repo
            .quarantine_active_deployment(second_deployment, "STALE", request, 60)
            .unwrap()
    );
}
