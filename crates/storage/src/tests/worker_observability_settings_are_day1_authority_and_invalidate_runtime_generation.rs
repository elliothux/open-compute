use super::*;
use crate::{DeploymentSource, WorkerObservabilityPatch};

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
            worker.route_generation,
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
    assert_eq!(
        repo.update_observability_settings(
            account,
            worker.id,
            worker.route_generation,
            &UpdateWorkerObservabilitySettings {
                enabled: true,
                head_sampling_rate: None,
                logs_enabled: true,
                logs_head_sampling_rate: None,
                invocation_logs: true,
                persist: true,
            },
            request,
            3,
        )
        .unwrap_err()
        .code(),
        ErrorCode::IdempotencyConflict
    );
    assert_eq!(
        repo.get_observability_settings(account, worker.id)
            .unwrap()
            .generation,
        2
    );

    let version = insert_ready(&repo, account, worker.id, [7; 32], request, 3);
    let patch = WorkerObservabilityPatch {
        logs_enabled: Some(false),
        persist: Some(false),
        ..WorkerObservabilityPatch::default()
    };
    let (published, _) = repo
        .create_deployment_checked(
            account,
            worker.id,
            version,
            None,
            Some(2),
            DeploymentSource::ScriptUpload,
            &BTreeMap::new(),
            Some(&patch),
            request,
            5,
            open_compute_core::StartupId::generate(),
        )
        .unwrap();
    assert_eq!(published.route_generation, 3);
    let published_settings = repo.get_observability_settings(account, worker.id).unwrap();
    assert_eq!(published_settings.generation, 3);
    assert!(!published_settings.logs_enabled);
    assert!(!published_settings.persist);

    let rejected = WorkerObservabilityPatch {
        enabled: Some(false),
        ..WorkerObservabilityPatch::default()
    };
    assert!(
        repo.create_deployment_checked(
            account,
            worker.id,
            version,
            Some(version),
            Some(2),
            DeploymentSource::ScriptUpload,
            &BTreeMap::new(),
            Some(&rejected),
            request,
            6,
            open_compute_core::StartupId::generate(),
        )
        .is_err()
    );
    assert_eq!(
        repo.get_observability_settings(account, worker.id).unwrap(),
        published_settings
    );
    assert_eq!(
        repo.get_worker(account, worker.id)
            .unwrap()
            .route_generation,
        3
    );
}
