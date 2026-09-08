use super::*;

#[tokio::test]
async fn cache_symlink_rejected_then_valid_refetch() {
    let mock = MockS3::spawn("open-compute").await;
    let store = ArtifactStore::new(client_for(&mock).await);
    let payload = Bytes::from_static(b"symlink-target-ok");
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
    let outside = tmp.path().join("outside-target");
    write_mode(&outside, "do-not-touch", 0o600);
    let dest = cache_entry_path(tmp.path(), &digest);
    fs::create_dir_all(dest.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(&outside, &dest).unwrap();
    let cache = ArtifactCache::open(
        tmp.path().to_path_buf(),
        cache_config(4096),
        StartupId::generate(),
    )
    .unwrap();
    let mut pin = cache.acquire(&store, &r).await.unwrap();
    assert_eq!(pin.read_all().unwrap(), payload.as_ref());
    assert_eq!(fs::read(&outside).unwrap(), b"do-not-touch");
    assert_eq!(fs::read(&dest).unwrap(), payload.as_ref());
}
