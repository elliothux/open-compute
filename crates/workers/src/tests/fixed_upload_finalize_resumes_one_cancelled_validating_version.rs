use super::*;

#[tokio::test]
async fn fixed_upload_finalize_resumes_one_cancelled_validating_version() {
    let temp = tempfile::tempdir().unwrap();
    let storage = Arc::new(
        PlatformStorage::bootstrap(&storage_config(&temp.path().join("data")), &SystemClock)
            .unwrap(),
    );
    let account = storage.identity().default_account_id;
    let worker = WorkerRepository::new(storage.db())
        .create_worker(
            account,
            "resume-upload",
            RequestId::generate(),
            1,
            1_000_000,
        )
        .unwrap()
        .0;
    let mock = MockS3::spawn("open-compute").await;
    let artifacts = artifact_store(&mock);
    let version_id = VersionId::generate();
    let request = version_request(account, worker.id, "upload-resume", "secret");
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let started = Arc::new(std::sync::Mutex::new(Some(started_tx)));
    let blocking_validator: Arc<dyn RuntimeValidator> = Arc::new({
        let started = started.clone();
        move |_: ValidationCandidate| {
            let started = started.lock().unwrap().take();
            async move {
                if let Some(started) = started {
                    let _ = started.send(());
                }
                std::future::pending::<Result<(), open_compute_core::PlatformError>>().await
            }
        }
    });
    let first_storage = storage.clone();
    let first_artifacts = artifacts.clone();
    let first_request = request.clone();
    let attempt = tokio::spawn(async move {
        VersionController::new(
            &first_storage,
            first_artifacts,
            blocking_validator,
            BundleLimits::default(),
        )
        .finalize_upload(first_request, version_id)
        .await
    });
    started_rx.await.unwrap();
    attempt.abort();
    assert!(attempt.await.unwrap_err().is_cancelled());
    let stranded = WorkerRepository::new(storage.db())
        .get_version(account, worker.id, version_id)
        .unwrap();
    assert_eq!(stranded.state, VersionState::Validating);
    let probe = RuntimeSource::new(storage.clone(), artifacts.clone(), BundleLimits::default())
        .resolve(
            &loader_key(account, worker.id, version_id),
            &hex::encode(stranded.worker_code_sha256),
            RuntimeScope::Probe,
        )
        .await
        .unwrap();
    assert!(probe.secrets.is_empty());

    let recovered = VersionController::new(
        &storage,
        artifacts,
        Arc::new(AcceptAllValidator),
        BundleLimits::default(),
    )
    .finalize_upload(request, version_id)
    .await
    .unwrap();
    let CreateVersionOutcome::Applied(result) = recovered else {
        panic!("cancelled finalize must complete its fixed version");
    };
    assert_eq!(result.version.id, version_id);
    assert_eq!(result.version.state, VersionState::Ready);
    assert_eq!(
        WorkerRepository::new(storage.db())
            .list_versions(account, worker.id)
            .unwrap()
            .len(),
        1
    );
}
