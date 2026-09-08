use super::*;

#[tokio::test]
async fn cache_partial_cleanup_on_open() {
    let tmp = TempDir::new().unwrap();
    let shard = tmp.path().join("sha256").join("ab");
    fs::create_dir_all(&shard).unwrap();
    let stale = shard.join(format!(".partial.{}.dead", StartupId::generate()));
    write_mode(&stale, "partial", 0o600);
    let old = SystemTime::now() - Duration::from_secs(10);
    OpenOptions::new()
        .write(true)
        .open(&stale)
        .unwrap()
        .set_modified(old)
        .unwrap();
    ArtifactCache::open(
        tmp.path().to_path_buf(),
        cache_config(40),
        StartupId::generate(),
    )
    .unwrap();
    assert!(!stale.exists());
}
