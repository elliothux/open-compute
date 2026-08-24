use super::*;
use std::os::unix::fs::symlink;
use tempfile::TempDir;

#[test]
fn cache_path_index_and_evict_target_validation_matrix() {
    let temp = TempDir::new().unwrap();
    let digest = hex::encode(Sha256::digest(b"cached"));
    assert_eq!(
        cache_path(temp.path(), &digest),
        temp.path()
            .join("sha256")
            .join(&digest[..2])
            .join(&digest[2..])
    );
    assert!(!is_safe_evict_target(&temp.path().join("missing")));
    let directory = temp.path().join("directory");
    fs::create_dir(&directory).unwrap();
    assert!(!is_safe_evict_target(&directory));
    let hidden = temp.path().join(".partial.value");
    fs::write(&hidden, b"x").unwrap();
    assert!(!is_safe_evict_target(&hidden));
    let regular = temp.path().join("regular");
    fs::write(&regular, b"x").unwrap();
    assert!(is_safe_evict_target(&regular));
    let link = temp.path().join("link");
    symlink(&regular, &link).unwrap();
    assert!(!is_safe_evict_target(&link));

    assert!(validate_cache_root(Path::new("relative")).is_err());
    assert!(validate_cache_root(&temp.path().join("child/../root")).is_err());

    let partial = temp.path().join("partial");
    fs::write(&partial, b"partial").unwrap();
    drop(PartialGuard {
        path: partial.clone(),
        persist: false,
    });
    assert!(!partial.exists());
    fs::write(&partial, b"keep").unwrap();
    drop(PartialGuard {
        path: partial.clone(),
        persist: true,
    });
    assert!(partial.exists());
}

#[test]
fn cleanup_and_index_ignore_untrusted_shapes() {
    let temp = TempDir::new().unwrap();
    let missing = temp.path().join("missing");
    cleanup_stale_partials(&missing, Duration::ZERO);
    assert!(rebuild_index(&missing).entries.is_empty());

    let sha_root = temp.path().join("sha256");
    fs::create_dir(&sha_root).unwrap();
    fs::write(sha_root.join("regular-shard"), b"ignored").unwrap();
    let outside = temp.path().join("outside");
    fs::create_dir(&outside).unwrap();
    symlink(&outside, sha_root.join("aa")).unwrap();
    let short = sha_root.join("x");
    fs::create_dir(&short).unwrap();

    let shard = sha_root.join("bb");
    fs::create_dir(&shard).unwrap();
    fs::write(shard.join("short"), b"ignored").unwrap();
    fs::write(shard.join(".".to_owned() + &"x".repeat(61)), b"ignored").unwrap();
    fs::write(shard.join("z".repeat(62)), b"invalid digest").unwrap();
    let valid_rest = "b".repeat(62);
    fs::create_dir(shard.join(&valid_rest)).unwrap();
    let link_rest = format!("{}c", "b".repeat(61));
    symlink(&outside, shard.join(link_rest)).unwrap();

    cleanup_stale_partials(&sha_root, Duration::ZERO);
    assert!(rebuild_index(&sha_root).entries.is_empty());
}

#[test]
fn cache_filesystem_and_hash_helpers_cover_valid_and_error_paths() {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("cache");
    ensure_real_dir(&root).unwrap();
    ensure_real_dir(&root).unwrap();
    ensure_child_dir(&root, "sha256").unwrap();
    ensure_child_dir(&root, "sha256").unwrap();
    assert!(open_dir_nofollow(&root, false).is_ok());
    assert!(open_dir_nofollow(&root.join("missing"), false).is_err());

    let blocked = root.join("blocked");
    fs::write(&blocked, b"not a directory").unwrap();
    assert!(ensure_child_dir(&root, "blocked").is_err());

    let sha_root = root.join("sha256");
    let digest = hex::encode(Sha256::digest(b"cached bytes"));
    let shard = sha_root.join(&digest[..2]);
    fs::create_dir(&shard).unwrap();
    let entry = shard.join(&digest[2..]);
    fs::write(&entry, b"cached bytes").unwrap();
    let index = rebuild_index(&sha_root);
    assert_eq!(index.total_bytes, 12);
    assert!(index.entries.contains_key(&digest));

    let artifact = ArtifactRef::new(1, &digest, 12).unwrap();
    let mut file = File::open(&entry).unwrap();
    hash_fd(&mut file, &artifact).unwrap();
    let wrong_size = ArtifactRef::new(1, &digest, 11).unwrap();
    let mut file = File::open(&entry).unwrap();
    assert_eq!(
        hash_fd(&mut file, &wrong_size).unwrap_err().code(),
        ErrorCode::CacheEntryCorrupt
    );
    let wrong_digest = ArtifactRef::new(1, &"00".repeat(32), 12).unwrap();
    let mut file = File::open(&entry).unwrap();
    assert_eq!(
        hash_fd(&mut file, &wrong_digest).unwrap_err().code(),
        ErrorCode::CacheEntryCorrupt
    );

    let partial = shard.join(".partial.old");
    fs::write(&partial, b"partial").unwrap();
    std::thread::sleep(Duration::from_millis(2));
    cleanup_stale_partials(&sha_root, Duration::ZERO);
    assert!(!partial.exists());
    fsync_dir(&root).unwrap();
}
