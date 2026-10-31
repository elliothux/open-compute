//! SQL authority changes and concurrent uploads at the preparation commit boundary.

use super::*;

#[tokio::test]
async fn retained_restore_rechecks_version_authority_at_the_commit_fence() {
    let base =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.temp/prepared-python-run");
    fs::create_dir_all(&base).unwrap();
    let evidence = recovery::Evidence(Some(
        tempfile::Builder::new()
            .prefix("publication-")
            .tempdir_in(fs::canonicalize(base).unwrap())
            .unwrap(),
    ));
    let config = storage_config(&evidence.0.as_ref().unwrap().path().join("data"));
    let storage = PlatformStorage::bootstrap(&config, &SystemClock).unwrap();
    let instance = storage.identity().instance_id;
    let repository = WorkerRepository::new(storage.db());
    let worker = repository
        .create_worker(instance, "publication-fence", RequestId::generate(), 1, 100)
        .unwrap()
        .0
        .id;
    let mock = MockS3::spawn("open-compute").await;
    let artifacts = artifact_store(&mock);
    let snapshot = SecretBytes::new(b"opaque snapshot bytes for authority".to_vec());
    let active_identity = create_candidate(&storage, worker, "main.py");
    let active_record =
        publish_prepared_python(&storage, &artifacts, &active_identity, &snapshot, 2)
            .await
            .unwrap();
    repository
        .mark_ready(active_identity.version_id, 3)
        .unwrap();
    let active = repository
        .promote(
            instance,
            worker,
            active_identity.version_id,
            None,
            RequestId::generate(),
            4,
        )
        .unwrap();

    for deleting in [false, true] {
        let identity = create_candidate(&storage, worker, "main.py");
        let record = publish_prepared_python(&storage, &artifacts, &identity, &snapshot, 5)
            .await
            .unwrap();
        if deleting {
            repository.mark_ready(identity.version_id, 6).unwrap();
        }
        let objects = mock.object_count();
        let error = python_artifact::publish_prepared_python(
            &storage,
            &artifacts,
            &identity,
            &snapshot,
            7,
            |commit| {
                // Object verification finished, but persisted admission changed
                // before the synchronous generation/authority fence was acquired.
                if deleting {
                    repository.begin_version_delete(instance, worker, identity.version_id)?;
                } else {
                    repository.mark_rejected(
                        identity.version_id,
                        VersionState::Validating,
                        ErrorCode::BundleRuntimeInvalid,
                        7,
                    )?;
                }
                commit()
            },
        )
        .await
        .unwrap_err();
        assert_eq!(error.code(), ErrorCode::VersionNotReady);
        assert_eq!(
            mock.object_count(),
            objects,
            "retained restore does not upload again"
        );
        assert_eq!(repository.get_worker(instance, worker).unwrap(), active);
        assert_eq!(
            repository
                .get_version(instance, worker, identity.version_id)
                .unwrap()
                .state,
            if deleting {
                VersionState::Deleting
            } else {
                VersionState::Rejected
            },
        );
        let connection = rusqlite::Connection::open_with_flags(
            config.path.join("control.sqlite"),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();
        let digest: Vec<u8> = connection
            .query_row(
                "SELECT artifact_sha256 FROM version_python_prepared WHERE version_id=?1",
                [identity.version_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(digest, record.artifact_sha256);
    }
    drop(storage);
    let storage = PlatformStorage::bootstrap(&config, &SystemClock).unwrap();
    assert_eq!(
        WorkerRepository::new(storage.db())
            .get_worker(instance, worker)
            .unwrap(),
        active
    );
    assert_eq!(
        restore_prepared_python(
            &active_record,
            &active_identity,
            &artifacts,
            storage.crypto()
        )
        .await
        .unwrap()
        .expose(),
        snapshot.expose(),
    );
}

#[tokio::test]
async fn concurrent_prepared_uploads_keep_one_sql_identity_and_collect_only_the_loser() {
    let base =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.temp/prepared-python-run");
    fs::create_dir_all(&base).unwrap();
    let evidence = recovery::Evidence(Some(
        tempfile::Builder::new()
            .prefix("concurrent-")
            .tempdir_in(fs::canonicalize(base).unwrap())
            .unwrap(),
    ));
    let config = storage_config(&evidence.0.as_ref().unwrap().path().join("data"));
    let storage = PlatformStorage::bootstrap(&config, &SystemClock).unwrap();
    let instance = storage.identity().instance_id;
    let repository = WorkerRepository::new(storage.db());
    let worker = repository
        .create_worker(
            instance,
            "publication-concurrent",
            RequestId::generate(),
            1,
            100,
        )
        .unwrap()
        .0
        .id;
    let identity = create_candidate(&storage, worker, "main.py");
    let mock = MockS3::spawn("open-compute").await;
    let artifacts = artifact_store(&mock);
    let first = SecretBytes::new(b"first opaque native snapshot".to_vec());
    let second = SecretBytes::new(b"second opaque native snapshot".to_vec());
    // Both publishers read the unprepared Version before either can finish
    // its object preflight. This is an explicit barrier, not a timing sleep.
    mock.synchronize_next_heads(2);
    let (first_result, second_result) = tokio::time::timeout(Duration::from_secs(10), async {
        tokio::join!(
            publish_prepared_python(&storage, &artifacts, &identity, &first, 2),
            publish_prepared_python(&storage, &artifacts, &identity, &second, 3),
        )
    })
    .await
    .unwrap();
    let (published, expected, rejected) = match (first_result, second_result) {
        (Ok(record), Err(error)) => (record, &first, error),
        (Err(error), Ok(record)) => (record, &second, error),
        outcome => panic!("exactly one SQL publication must win: {outcome:?}"),
    };
    assert_eq!(rejected.code(), ErrorCode::VersionInvariantViolation);
    assert_eq!(mock.object_count(), 2);
    assert_eq!(
        repository
            .version_snapshot(instance, worker, identity.version_id, true)
            .unwrap()
            .python_prepared,
        Some(published.clone()),
    );
    repository.mark_ready(identity.version_id, 4).unwrap();
    drop(storage);
    let storage = PlatformStorage::bootstrap(&config, &SystemClock).unwrap();
    let repository = WorkerRepository::new(storage.db());
    let fence = artifacts.fence_version_gc().await;
    let references = repository
        .referenced_artifacts()
        .unwrap()
        .into_iter()
        .map(|(digest, size)| ArtifactRef::new(1, &hex::encode(digest), size).unwrap())
        .collect();
    assert_eq!(
        artifacts
            .gc_unreferenced(
                &fence,
                &references,
                SystemTime::now() + Duration::from_secs(60),
            )
            .await
            .unwrap(),
        1
    );
    drop(fence);
    assert_eq!(mock.object_count(), 1);
    assert_eq!(
        restore_prepared_python(&published, &identity, &artifacts, storage.crypto())
            .await
            .unwrap()
            .expose(),
        expected.expose(),
    );
    assert_eq!(
        publish_prepared_python(&storage, &artifacts, &identity, &first, 5)
            .await
            .unwrap(),
        published,
    );
    assert_eq!(
        mock.object_count(),
        1,
        "retry restores the winning immutable bytes"
    );
}

#[tokio::test]
async fn retained_restore_does_not_accept_or_repair_changed_prepared_authority() {
    let base =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.temp/prepared-python-run");
    fs::create_dir_all(&base).unwrap();
    let evidence = recovery::Evidence(Some(
        tempfile::Builder::new()
            .prefix("corrupt-publication-")
            .tempdir_in(fs::canonicalize(base).unwrap())
            .unwrap(),
    ));
    let config = storage_config(&evidence.0.as_ref().unwrap().path().join("data"));
    let storage = PlatformStorage::bootstrap(&config, &SystemClock).unwrap();
    let instance = storage.identity().instance_id;
    let repository = WorkerRepository::new(storage.db());
    let worker = repository
        .create_worker(
            instance,
            "publication-corrupt",
            RequestId::generate(),
            1,
            100,
        )
        .unwrap()
        .0
        .id;
    let identity = create_candidate(&storage, worker, "main.py");
    let mock = MockS3::spawn("open-compute").await;
    let artifacts = artifact_store(&mock);
    let bytes = SecretBytes::new(b"opaque native snapshot integrity".to_vec());
    let record = publish_prepared_python(&storage, &artifacts, &identity, &bytes, 2)
        .await
        .unwrap();
    let mut corrupted_identity = record.prepared_identity_sha256;
    corrupted_identity[0] ^= 1;
    let error = python_artifact::publish_prepared_python(
        &storage, &artifacts, &identity, &bytes, 3,
        |commit| {
            let mut connection = rusqlite::Connection::open(config.path.join("control.sqlite"))
                .unwrap();
            let update = "UPDATE version_python_prepared SET prepared_identity_sha256=?1 WHERE version_id=?2";
            assert!(connection.execute(update, rusqlite::params![
                corrupted_identity.as_slice(), identity.version_id,
            ]).is_err(), "the production immutability guard remains enabled");
            let guard: String = connection.query_row(
                "SELECT sql FROM sqlite_master WHERE name='version_python_prepared_update_guard'",
                [], |row| row.get(0),
            ).unwrap();
            // Persist corruption only in this owned fixture, restoring the exact
            // trigger in the same transaction before testing the authority reader.
            let transaction = connection.transaction().unwrap();
            transaction.execute_batch("DROP TRIGGER version_python_prepared_update_guard")
                .unwrap();
            assert_eq!(transaction.execute(update, rusqlite::params![
                corrupted_identity.as_slice(), identity.version_id,
            ]).unwrap(), 1);
            transaction.execute_batch(&guard).unwrap();
            transaction.commit().unwrap();
            commit()
        },
    ).await.unwrap_err();
    assert_eq!(error.code(), ErrorCode::VersionInvariantViolation);
    assert_eq!(mock.object_count(), 1);
    drop(storage);
    let storage = PlatformStorage::bootstrap(&config, &SystemClock).unwrap();
    let repository = WorkerRepository::new(storage.db());
    let retained = repository
        .version_snapshot(instance, worker, identity.version_id, true)
        .unwrap()
        .python_prepared
        .unwrap();
    assert_eq!(retained.prepared_identity_sha256, corrupted_identity);
    assert_eq!(retained.artifact_sha256, record.artifact_sha256);
    mock.clear_recorded();
    assert_eq!(
        publish_prepared_python(&storage, &artifacts, &identity, &bytes, 4)
            .await
            .unwrap_err()
            .code(),
        ErrorCode::VersionInvariantViolation,
    );
    assert!(
        mock.recorded().is_empty(),
        "corrupt metadata fails before S3 access"
    );
    assert_eq!(
        repository
            .version_snapshot(instance, worker, identity.version_id, true)
            .unwrap()
            .python_prepared,
        Some(retained)
    );
    assert_eq!(
        repository
            .get_version(instance, worker, identity.version_id)
            .unwrap()
            .state,
        VersionState::Validating
    );
    assert_eq!(
        repository
            .get_worker(instance, worker)
            .unwrap()
            .active_version_id,
        None
    );
}
