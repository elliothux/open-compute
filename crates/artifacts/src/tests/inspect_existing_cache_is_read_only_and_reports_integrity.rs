use super::*;

#[test]
fn inspect_existing_cache_is_read_only_and_reports_integrity() {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("cache");
    assert!(ArtifactCache::inspect_existing(root.clone()).is_err());
    fs::write(&root, b"not a directory").unwrap();
    assert!(ArtifactCache::inspect_existing(root.clone()).is_err());
    fs::remove_file(&root).unwrap();
    fs::create_dir(&root).unwrap();

    let empty = ArtifactCache::inspect_existing(root.clone()).unwrap();
    assert_eq!(empty.entry_count(), 0);
    let empty_sample = empty.sample_integrity().unwrap();
    assert_eq!(empty_sample.entries, 0);
    assert_eq!(empty_sample.bytes, 0);
    assert!(!empty_sample.corrupt);
    assert!(format!("{empty_sample:?}").contains("entries"));

    let sha_root = root.join("sha256");
    fs::write(&sha_root, b"not a directory").unwrap();
    assert!(ArtifactCache::inspect_existing(root.clone()).is_err());
    fs::remove_file(&sha_root).unwrap();
    let outside = temp.path().join("outside");
    fs::create_dir(&outside).unwrap();
    std::os::unix::fs::symlink(&outside, &sha_root).unwrap();
    assert!(ArtifactCache::inspect_existing(root.clone()).is_err());
    fs::remove_file(&sha_root).unwrap();

    fs::create_dir(&sha_root).unwrap();
    fs::write(sha_root.join("regular-shard"), b"ignored").unwrap();
    fs::create_dir(sha_root.join("x")).unwrap();
    let digest = hex::encode(Sha256::digest(b"cached"));
    let shard = sha_root.join(&digest[..2]);
    fs::create_dir(&shard).unwrap();
    fs::write(shard.join(&digest[2..]), b"cached").unwrap();
    fs::write(shard.join("short"), b"ignored").unwrap();
    let cache = ArtifactCache::inspect_existing(root.clone()).unwrap();
    assert_eq!(cache.entry_count(), 1);
    let sample = cache.sample_integrity().unwrap();
    assert_eq!(sample.entries, 1);
    assert_eq!(sample.bytes, 6);
    assert!(!sample.corrupt);

    fs::write(shard.join(&digest[2..]), b"broken").unwrap();
    assert!(cache.sample_integrity().unwrap().corrupt);
    fs::remove_file(shard.join(&digest[2..])).unwrap();
    assert!(cache.sample_integrity().unwrap().corrupt);
}
