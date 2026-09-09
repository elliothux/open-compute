use super::*;

#[tokio::test]
async fn artifact_listing_skips_invalid_keys_and_gc_respects_missing_time() {
    let mock = MockS3::spawn("open-compute").await;
    let store = ArtifactStore::new(client_for(&mock).await);
    mock.put_raw("system/artifacts/v1/sha256/not-valid", b"bad".to_vec());
    let payload = b"candidate";
    let digest = hex::encode(Sha256::digest(payload));
    let artifact = ArtifactRef::new(1, &digest, payload.len() as u64).unwrap();
    mock.put_raw(&artifact.physical_key("system/"), payload.to_vec());

    let candidates = store.list_candidates().await.unwrap();
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].artifact, artifact);
    let mut referenced = HashSet::new();
    referenced.insert(artifact.clone());
    assert_eq!(
        store
            .gc_unreferenced(
                &store.fence_version_gc().await,
                &referenced,
                SystemTime::now(),
            )
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        store
            .gc_unreferenced(
                &store.fence_version_gc().await,
                &HashSet::new(),
                SystemTime::UNIX_EPOCH,
            )
            .await
            .unwrap(),
        0
    );

    mock.set_omit_last_modified(true);
    assert_eq!(
        store
            .gc_unreferenced(
                &store.fence_version_gc().await,
                &HashSet::new(),
                SystemTime::now(),
            )
            .await
            .unwrap(),
        0
    );
}
