use super::*;

#[tokio::test]
async fn worker_artifact_gc_skips_when_final_reference_snapshot_fails() {
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
    let payload = bytes::Bytes::from_static(b"unreferenced-old-artifact");
    let digest = hex::encode(sha2::Sha256::digest(&payload));
    store
        .put_verified(
            futures::stream::iter(vec![Ok::<_, std::io::Error>(payload.clone())]),
            &digest,
            payload.len() as u64,
        )
        .await
        .unwrap();
    storage.db().set_foreign_keys_for_test(false).unwrap();
    let mut workers = loaded.config.workers;
    workers.artifact_gc_grace_ms = 0;
    gc_worker_artifacts(
        &storage,
        &store,
        &workers,
        &crate::snapshot_pins::SnapshotPins::empty(),
        None,
    )
    .await
    .unwrap_err();
    assert_eq!(mock.object_count(), 2);
    storage.db().set_foreign_keys_for_test(true).unwrap();
}
