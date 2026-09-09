use super::*;

#[tokio::test]
async fn cache_corrupt_local_and_s3_fails_once_and_cleans() {
    let mock = MockS3::spawn("open-compute").await;
    let store = ArtifactStore::new(client_for(&mock).await);
    let payload = Bytes::from_static(b"will-be-wrong!");
    let digest = hex::encode(Sha256::digest(&payload));
    let r = store
        .put_verified(
            stream::iter(vec![Ok::<Bytes, std::io::Error>(payload.clone())]),
            &digest,
            payload.len() as u64,
        )
        .await
        .unwrap();
    let tmp = TempDir::new().unwrap();
    let cache = ArtifactCache::open(
        tmp.path().to_path_buf(),
        cache_config(4096),
        StartupId::generate(),
    )
    .unwrap();
    cache.acquire(&store, &r).await.unwrap();
    let dest = cache_entry_path(tmp.path(), &digest);
    fs::write(&dest, vec![b'Y'; payload.len()]).unwrap();
    let cache = ArtifactCache::open(
        tmp.path().to_path_buf(),
        cache_config(4096),
        StartupId::generate(),
    )
    .unwrap();
    mock.set_fault(Fault::CorruptBody);
    let gets_before = mock.artifact_gets();
    let err = cache.acquire(&store, &r).await.unwrap_err();
    assert_eq!(err.code(), ErrorCode::ArtifactIntegrityError);
    assert_eq!(mock.artifact_gets(), gets_before + 1);
    assert!(!dest.exists());
    assert!(list_partials(tmp.path()).is_empty());
}
