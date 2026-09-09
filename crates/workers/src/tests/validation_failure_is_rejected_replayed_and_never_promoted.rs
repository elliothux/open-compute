use super::*;

#[tokio::test]
async fn validation_failure_is_rejected_replayed_and_never_promoted() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("data");
    let storage = PlatformStorage::bootstrap(&storage_config(&root), &SystemClock).unwrap();
    let account = storage.identity().default_account_id;
    let repo = WorkerRepository::new(storage.db());
    let (worker, _) = repo
        .create_worker(
            account,
            "invalid-runtime",
            RequestId::generate(),
            1,
            1_000_000,
        )
        .unwrap();
    let mock = MockS3::spawn("open-compute").await;
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let seen = calls.clone();
    let validator: Arc<dyn RuntimeValidator> = Arc::new(move |_: ValidationCandidate| {
        seen.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        async {
            Err(open_compute_core::PlatformError::new(
                ErrorCode::BundleRuntimeInvalid,
                "synthetic safe validator failure",
            ))
        }
    });
    let artifacts = artifact_store(&mock);
    let controller = VersionController::new(
        &storage,
        artifacts.clone(),
        validator.clone(),
        BundleLimits::default(),
    );
    let request = version_request(account, worker.id, "rejected-key", "rejected-secret");
    assert_eq!(
        controller
            .create_version(request.clone())
            .await
            .unwrap_err()
            .code(),
        ErrorCode::BundleRuntimeInvalid
    );
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    let versions = repo.list_versions(account, worker.id).unwrap();
    assert_eq!(versions.len(), 1);
    assert_eq!(versions[0].state, VersionState::Rejected);
    assert_eq!(
        repo.get_worker(account, worker.id)
            .unwrap()
            .active_version_id,
        None
    );

    drop(controller);
    drop(storage);
    let restarted = PlatformStorage::bootstrap(&storage_config(&root), &SystemClock).unwrap();
    let restarted_controller =
        VersionController::new(&restarted, artifacts, validator, BundleLimits::default());
    assert_eq!(
        restarted_controller
            .create_version(request)
            .await
            .unwrap_err()
            .code(),
        ErrorCode::BundleRuntimeInvalid
    );
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    let restarted_repo = WorkerRepository::new(restarted.db());
    assert_eq!(
        restarted_repo
            .list_versions(account, worker.id)
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        restarted_repo
            .get_worker(account, worker.id)
            .unwrap()
            .active_version_id,
        None
    );
}
