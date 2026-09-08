use super::*;

#[tokio::test]
async fn cached_acquire_rejects_directory_and_size_mismatch_entries() {
    let temp = TempDir::new().unwrap();
    let cache = ArtifactCache::open(
        temp.path().join("cache"),
        cache_config(1024),
        StartupId::generate(),
    )
    .unwrap();
    let digest = "ab".repeat(32);
    let artifact = ArtifactRef::new(1, &digest, 1).unwrap();
    let path = cache_entry_path(&temp.path().join("cache"), &digest);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::create_dir(&path).unwrap();
    assert_eq!(
        cache.acquire_cached(&artifact).await.unwrap_err().code(),
        ErrorCode::CacheEntryCorrupt
    );
    fs::remove_dir(&path).unwrap();
    fs::write(&path, b"too long").unwrap();
    assert_eq!(
        cache.acquire_cached(&artifact).await.unwrap_err().code(),
        ErrorCode::CacheEntryCorrupt
    );
}
