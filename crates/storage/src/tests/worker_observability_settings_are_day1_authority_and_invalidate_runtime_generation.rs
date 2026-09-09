use super::*;

#[test]
fn worker_observability_settings_are_day1_authority_and_invalidate_runtime_generation() {
    let (_tmp, root) = unique_root();
    let storage = PlatformStorage::bootstrap(&storage_config(&root), &SystemClock).unwrap();
    let account = storage.identity().default_account_id;
    let repo = WorkerRepository::new(storage.db());
    let request = open_compute_core::RequestId::generate();
    let (worker, _) = repo
        .create_worker(account, "observability", request, 1, 10)
        .unwrap();
    let initial = repo.get_observability_settings(account, worker.id).unwrap();
    assert!(initial.enabled && initial.logs_enabled && initial.invocation_logs && initial.persist);
    assert_eq!(initial.generation, 1);

    let updated = repo
        .update_observability_settings(
            account,
            worker.id,
            &UpdateWorkerObservabilitySettings {
                enabled: true,
                head_sampling_rate: Some(0.5),
                logs_enabled: true,
                logs_head_sampling_rate: Some(0.25),
                invocation_logs: false,
                persist: true,
            },
            open_compute_core::RequestId::generate(),
            2,
        )
        .unwrap();
    assert_eq!(updated.generation, 2);
    assert_eq!(updated.effective_head_sampling_rate(), 0.25);
    assert!(!updated.invocation_logs);
    assert_eq!(
        repo.get_worker(account, worker.id)
            .unwrap()
            .route_generation,
        2
    );
}
