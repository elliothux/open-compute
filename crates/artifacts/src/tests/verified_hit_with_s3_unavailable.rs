use super::*;

#[tokio::test]
async fn verified_hit_with_s3_unavailable() {
    let mock = MockS3::spawn("open-compute").await;
    let store = ArtifactStore::new(client_for(&mock).await);
    let payload = Bytes::from_static(b"cached-bytes");
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
    let mut pin = cache.acquire(&store, &r).await.unwrap();
    assert_eq!(pin.read_all().unwrap(), payload.as_ref());
    drop(pin);
    mock.set_fault(Fault::ServerError);
    let gets = mock.artifact_gets();
    let mut hit = cache.acquire(&store, &r).await.unwrap();
    assert_eq!(hit.read_all().unwrap(), payload.as_ref());
    assert_eq!(mock.artifact_gets(), gets);
    let mut cached = cache.acquire_cached(&r).await.unwrap();
    assert_eq!(cached.read_all().unwrap(), payload.as_ref());
    assert_eq!(mock.artifact_gets(), gets);
}
