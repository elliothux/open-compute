use super::*;

#[tokio::test]
async fn eviction_stops_at_low_watermark_with_lru_entries_remaining() {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("cache");
    for byte in *b"abcd" {
        let body = vec![byte; 3];
        let digest = hex::encode(Sha256::digest(&body));
        let path = cache_entry_path(&root, &digest);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, body).unwrap();
    }
    let cache = ArtifactCache::open(
        root,
        CacheConfig {
            max_bytes: 10,
            high_watermark_ratio: 0.9,
            low_watermark_ratio: 0.5,
            partial_grace_ms: 0,
            max_artifact_bytes: 1024,
        },
        StartupId::generate(),
    )
    .unwrap();
    assert_eq!(cache.entry_count(), 4);
    cache.evict_if_needed().await.unwrap();
    assert_eq!(cache.entry_count(), 1);
}
