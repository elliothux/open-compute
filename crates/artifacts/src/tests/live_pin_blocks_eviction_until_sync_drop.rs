use super::*;

#[tokio::test]
async fn live_pin_blocks_eviction_until_sync_drop() {
    let mock = MockS3::spawn("open-compute").await;
    let store = ArtifactStore::new(client_for(&mock).await);
    let p1 = Bytes::from_static(b"aaaaaaaaaaaaaaaa");
    let p2 = Bytes::from_static(b"bbbbbbbbbbbbbbbb");
    let d1 = hex::encode(Sha256::digest(&p1));
    let d2 = hex::encode(Sha256::digest(&p2));
    let r1 = store
        .put_verified(
            stream::iter(vec![Ok::<Bytes, std::io::Error>(p1.clone())]),
            &d1,
            p1.len() as u64,
        )
        .await
        .unwrap();
    let r2 = store
        .put_verified(
            stream::iter(vec![Ok::<Bytes, std::io::Error>(p2.clone())]),
            &d2,
            p2.len() as u64,
        )
        .await
        .unwrap();
    let tmp = TempDir::new().unwrap();
    let cache = ArtifactCache::open(
        tmp.path().to_path_buf(),
        cache_config(16),
        StartupId::generate(),
    )
    .unwrap();
    let pin = cache.acquire(&store, &r1).await.unwrap();
    cache.acquire(&store, &r2).await.unwrap();
    cache.evict_if_needed().await.unwrap();
    assert!(cache_entry_path(tmp.path(), &d1).exists());
    drop(pin);
    cache.evict_if_needed().await.unwrap();
    assert!(!cache_entry_path(tmp.path(), &d1).exists());
}
