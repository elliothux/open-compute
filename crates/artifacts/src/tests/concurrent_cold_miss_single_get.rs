use super::*;

#[tokio::test]
async fn concurrent_cold_miss_single_get() {
    let mock = MockS3::spawn("open-compute").await;
    let store = ArtifactStore::new(client_for(&mock).await);
    let payload = Bytes::from_static(b"singleflight-body");
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
    let cache = Arc::new(
        ArtifactCache::open(
            tmp.path().to_path_buf(),
            cache_config(4096),
            StartupId::generate(),
        )
        .unwrap(),
    );
    let gets_before = mock.artifact_gets();
    let c1 = Arc::clone(&cache);
    let c2 = Arc::clone(&cache);
    let s1 = store.clone();
    let s2 = store.clone();
    let r1 = r.clone();
    let r2 = r.clone();
    let a = tokio::spawn(async move { c1.acquire(&s1, &r1).await });
    let b = tokio::spawn(async move { c2.acquire(&s2, &r2).await });
    let pa = a.await.unwrap().unwrap();
    let pb = b.await.unwrap().unwrap();
    drop(pa);
    drop(pb);
    assert_eq!(mock.artifact_gets(), gets_before + 1);
}
