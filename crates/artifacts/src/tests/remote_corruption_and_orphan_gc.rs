use super::*;

#[tokio::test]
async fn remote_corruption_and_orphan_gc() {
    let mock = MockS3::spawn("open-compute").await;
    let store = ArtifactStore::new(client_for(&mock).await);
    let payload = Bytes::from_static(b"gc-me");
    let digest = hex::encode(Sha256::digest(&payload));
    let r = store
        .put_verified(
            stream::iter(vec![Ok::<Bytes, std::io::Error>(payload.clone())]),
            &digest,
            5,
        )
        .await
        .unwrap();
    mock.corrupt_body(&r.physical_key("system/"));
    let err = store.open(&r).await.unwrap_err();
    assert_eq!(err.code(), ErrorCode::ArtifactIntegrityError);

    mock.put_raw("tenant/not-ours", b"x".to_vec());
    let candidates = store.list_candidates().await.unwrap();
    assert!(
        candidates
            .iter()
            .all(|c| c.artifact.sha256_hex().len() == 64)
    );
    let referenced = HashSet::new();
    let deleted = store
        .gc_unreferenced(
            &store.fence_version_gc().await,
            &referenced,
            SystemTime::now() + Duration::from_secs(1),
        )
        .await
        .unwrap();
    assert_eq!(deleted, 1);
    assert_eq!(mock.object_count(), 1);

    let mock2 = MockS3::spawn("open-compute").await;
    let store2 = ArtifactStore::new(client_for(&mock2).await);
    let payload2 = Bytes::from_static(b"keep-me");
    let digest2 = hex::encode(Sha256::digest(&payload2));
    store2
        .put_verified(
            stream::iter(vec![Ok::<Bytes, std::io::Error>(payload2.clone())]),
            &digest2,
            7,
        )
        .await
        .unwrap();
    mock2.set_omit_last_modified(true);
    let deleted2 = store2
        .gc_unreferenced(
            &store2.fence_version_gc().await,
            &HashSet::new(),
            SystemTime::now() + Duration::from_secs(1),
        )
        .await
        .unwrap();
    assert_eq!(deleted2, 0);
    assert_eq!(mock2.object_count(), 1);
}
