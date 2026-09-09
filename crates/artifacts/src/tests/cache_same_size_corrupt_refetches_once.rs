use super::*;

#[tokio::test]
async fn cache_same_size_corrupt_refetches_once() {
    let mock = MockS3::spawn("open-compute").await;
    let store = ArtifactStore::new(client_for(&mock).await);
    let payload = Bytes::from_static(b"correct-bytes!!");
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
    let corrupt = vec![b'X'; payload.len()];
    fs::write(&dest, &corrupt).unwrap();
    let cache = ArtifactCache::open(
        tmp.path().to_path_buf(),
        cache_config(4096),
        StartupId::generate(),
    )
    .unwrap();
    let gets_before = mock.artifact_gets();
    let mut pin = cache.acquire(&store, &r).await.unwrap();
    assert!(pin.file().metadata().unwrap().is_file());
    assert_eq!(pin.read_all().unwrap(), payload.as_ref());
    assert_eq!(mock.artifact_gets(), gets_before + 1);
    assert_eq!(fs::read(&dest).unwrap(), payload.as_ref());
}
