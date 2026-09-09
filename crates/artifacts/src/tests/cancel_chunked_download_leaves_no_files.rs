use super::*;

#[tokio::test]
async fn cancel_chunked_download_leaves_no_files() {
    let mock = MockS3::spawn("open-compute").await;
    mock.set_get_chunking(1, Duration::from_millis(80));
    let store = ArtifactStore::new(client_for(&mock).await);
    let payload = Bytes::from_static(b"slow-download!!");
    let digest = hex::encode(Sha256::digest(&payload));
    let r = store
        .put_verified(
            stream::iter(vec![Ok::<Bytes, std::io::Error>(payload.clone())]),
            &digest,
            payload.len() as u64,
        )
        .await
        .unwrap();
    mock.set_get_chunking(1, Duration::from_millis(80));
    let tmp = TempDir::new().unwrap();
    let cache = ArtifactCache::open(
        tmp.path().to_path_buf(),
        cache_config(4096),
        StartupId::generate(),
    )
    .unwrap();
    let handle = tokio::spawn(async move { cache.acquire(&store, &r).await });
    tokio::time::sleep(Duration::from_millis(120)).await;
    handle.abort();
    let _ = handle.await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(!cache_entry_path(tmp.path(), &digest).exists());
    assert!(list_partials(tmp.path()).is_empty());
}
