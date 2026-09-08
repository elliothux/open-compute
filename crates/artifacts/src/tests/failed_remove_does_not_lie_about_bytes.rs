use super::*;

#[tokio::test]
async fn failed_remove_does_not_lie_about_bytes() {
    let mock = MockS3::spawn("open-compute").await;
    let store = ArtifactStore::new(client_for(&mock).await);
    let payload = Bytes::from_static(b"keep-accounting");
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
        cache_config(payload.len() as u64),
        StartupId::generate(),
    )
    .unwrap();
    cache.acquire(&store, &r).await.unwrap();
    let before = cache.total_bytes().await;
    assert_eq!(before, payload.len() as u64);
    let shard = tmp.path().join("sha256").join(&digest[..2]);
    let orig = fs::metadata(&shard).unwrap().permissions();
    let mut ro = orig.clone();
    ro.set_mode(0o555);
    fs::set_permissions(&shard, ro).unwrap();
    cache.evict_if_needed().await.unwrap();
    assert_eq!(cache.total_bytes().await, before);
    assert!(cache_entry_path(tmp.path(), &digest).exists());
    fs::set_permissions(&shard, orig).unwrap();
}
