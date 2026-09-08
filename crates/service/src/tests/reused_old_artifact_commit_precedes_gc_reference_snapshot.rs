use super::*;

#[tokio::test]
async fn reused_old_artifact_commit_precedes_gc_reference_snapshot() {
    let (_dir, path, mock) = initialized_doctor_fixture().await;
    let loaded = load_fixture_platform_config(&path);
    let storage = Arc::new(
        open_compute_storage::PlatformStorage::bootstrap(
            &loaded.config.data,
            &open_compute_core::SystemClock,
        )
        .unwrap(),
    );
    let s3 = loaded.config.object_storage.as_s3().expect("S3 config");
    let credentials = resolve_fixture_s3_credentials(s3);
    let client = open_compute_artifacts::ObjectBackend::connect_s3(
        s3,
        &credentials,
        loaded.config.cache.max_artifact_bytes,
    )
    .unwrap();
    let store = open_compute_artifacts::ArtifactStore::new(client);
    let payload = bytes::Bytes::from_static(b"reused-old-artifact");
    let digest: [u8; 32] = sha2::Sha256::digest(&payload).into();
    let digest_hex = hex::encode(digest);
    store
        .put_verified(
            futures::stream::iter(vec![Ok::<_, std::io::Error>(payload.clone())]),
            &digest_hex,
            payload.len() as u64,
        )
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(2)).await;

    let repo = open_compute_storage::WorkerRepository::new(storage.db());
    let account = storage.identity().default_account_id;
    let (worker, _) = repo
        .create_worker(
            account,
            "gc-reference-fence",
            open_compute_core::RequestId::generate(),
            1,
            1_000_000,
        )
        .unwrap();
    let reservation = store.reserve_version_artifact().await;
    let mut workers = loaded.config.workers;
    workers.artifact_gc_grace_ms = 0;
    let gc_storage = storage.clone();
    let gc_store = store.clone();
    let mut gc = tokio::spawn(async move {
        gc_worker_artifacts(
            &gc_storage,
            &gc_store,
            &workers,
            &crate::snapshot_pins::SnapshotPins::empty(),
            None,
        )
        .await
        .unwrap();
    });
    assert!(
        tokio::time::timeout(Duration::from_millis(20), &mut gc)
            .await
            .is_err(),
        "GC must wait for the version commit reservation"
    );
    let version = open_compute_core::VersionId::generate();
    repo.insert_staging_version(
        &open_compute_storage::NewVersion {
            id: version,
            account_id: account,
            worker_id: worker.id,
            content_kind: open_compute_storage::VersionContentKind::Worker,
            artifact_sha256: Some(digest),
            artifact_size: Some(payload.len() as u64),
            artifact_schema_version: Some(1),
            main_module: Some("index.js".to_owned()),
            worker_code_sha256: [7; 32],
            compatibility_date: "2026-08-30".into(),
            compatibility_flags: Vec::new(),
            vars: std::collections::BTreeMap::new(),
            secrets: std::collections::BTreeMap::new(),
            request_id: open_compute_core::RequestId::generate(),
            now_ms: 2,
        },
        &open_compute_storage::NewVersionProducts::default(),
        1_000_000,
    )
    .unwrap();
    drop(reservation);
    tokio::time::timeout(Duration::from_secs(1), gc)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(mock.object_count(), 2);
}
