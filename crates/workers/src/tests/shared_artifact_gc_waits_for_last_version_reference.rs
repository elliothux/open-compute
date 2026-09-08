use super::*;

#[tokio::test]
async fn shared_artifact_gc_waits_for_last_version_reference() {
    let temp = tempfile::tempdir().unwrap();
    let storage = PlatformStorage::bootstrap(&storage_config(temp.path()), &SystemClock).unwrap();
    let mock = MockS3::spawn("open-compute").await;
    let artifacts = artifact_store(&mock);
    let account = storage.identity().default_account_id;
    let repo = WorkerRepository::new(storage.db());
    let request_id = RequestId::generate();
    let (first_worker, _) = repo
        .create_worker(account, "gc-first", request_id, 1, 1_000_000)
        .unwrap();
    let (second_worker, _) = repo
        .create_worker(account, "gc-second", request_id, 2, 1_000_000)
        .unwrap();
    let validator: Arc<dyn RuntimeValidator> = Arc::new(|_| async { Ok(()) });
    let controller = VersionController::new(
        &storage,
        artifacts.clone(),
        validator,
        BundleLimits::default(),
    );
    let mut first = version_request(account, first_worker.id, "gc-first", "same-secret");
    first.deployment_source = None;
    let mut second = version_request(account, second_worker.id, "gc-second", "same-secret");
    second.deployment_source = None;
    let first = match controller.create_version(first).await.unwrap() {
        CreateVersionOutcome::Applied(result) => result.version,
        CreateVersionOutcome::Replay(_) => panic!("unexpected replay"),
    };
    let second = match controller.create_version(second).await.unwrap() {
        CreateVersionOutcome::Applied(result) => result.version,
        CreateVersionOutcome::Replay(_) => panic!("unexpected replay"),
    };
    assert_eq!(first.artifact_sha256, second.artifact_sha256);
    assert_eq!(mock.object_count(), 1);
    let _ = repo.prune_expired_idempotency(i64::MAX, 100).unwrap();

    repo.begin_version_delete(account, first_worker.id, first.id)
        .unwrap();
    repo.finalize_version_delete(account, first_worker.id, first.id, request_id, 20)
        .unwrap();
    let referenced = repo
        .referenced_artifacts()
        .unwrap()
        .into_iter()
        .map(|(digest, size)| ArtifactRef::new(1, &hex::encode(digest), size).unwrap())
        .collect::<HashSet<_>>();
    assert_eq!(
        artifacts
            .gc_unreferenced(
                &artifacts.fence_version_gc().await,
                &referenced,
                SystemTime::now() + Duration::from_secs(1),
            )
            .await
            .unwrap(),
        0
    );
    assert_eq!(mock.object_count(), 1);

    repo.begin_version_delete(account, second_worker.id, second.id)
        .unwrap();
    repo.finalize_version_delete(account, second_worker.id, second.id, request_id, 21)
        .unwrap();
    assert_eq!(
        artifacts
            .gc_unreferenced(
                &artifacts.fence_version_gc().await,
                &HashSet::new(),
                SystemTime::now() + Duration::from_secs(1),
            )
            .await
            .unwrap(),
        1
    );
    assert_eq!(mock.object_count(), 0);
}
