use super::*;

#[test]
fn system_dashboard_worker_is_excluded_from_tenant_catalog_and_mutations() {
    let (_tmp, root) = unique_root();
    let storage = PlatformStorage::bootstrap(&storage_config(&root), &SystemClock).unwrap();
    let account = storage.identity().default_account_id;
    let repo = WorkerRepository::new(storage.db());
    let request = open_compute_core::RequestId::generate();

    assert_eq!(
        repo.create_worker(
            account,
            SYSTEM_DASHBOARD_WORKER_NAME,
            request,
            1,
            storage.hardening().max_workers_per_account,
        )
        .expect_err("reserved dashboard name")
        .code(),
        ErrorCode::WorkerNameConflict
    );

    let system_worker = repo
        .ensure_system_dashboard_worker(account, request, 1)
        .expect("system dashboard worker");
    assert_eq!(system_worker.name, SYSTEM_DASHBOARD_WORKER_NAME);
    assert!(
        repo.list_workers(account)
            .unwrap()
            .iter()
            .all(|worker| worker.id != system_worker.id)
    );
    assert_eq!(
        repo.get_tenant_worker(account, system_worker.id)
            .expect_err("tenant lookup")
            .code(),
        ErrorCode::WorkerNotFound
    );
    assert!(
        repo.get_system_owned_version(SystemOwnedVersionKind::Dashboard)
            .unwrap()
            .is_some()
    );
    assert_eq!(
        repo.get_version(account, system_worker.id, VersionId::generate())
            .expect_err("tenant version lookup")
            .code(),
        ErrorCode::WorkerNotFound
    );
}
