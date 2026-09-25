use super::*;
use std::os::unix::fs::symlink;

#[test]
fn runtime_clean_preserves_current_unknown_and_symlinked_packages() {
    let temp = tempfile::tempdir().unwrap();
    let cache = temp.path().join("cache");
    assert_eq!(
        clean_embedded_runtime_cache(&cache, true).unwrap(),
        RuntimeCacheCleanReport::default()
    );
    assert!(!cache.exists());

    let packages = cache.join("packages");
    let current = packages.join(payload::PAYLOAD_SHA256);
    let old = packages.join("ab".repeat(32));
    let unknown = packages.join("other-tool");
    let outside = temp.path().join("outside");
    fs::create_dir_all(&current).unwrap();
    fs::create_dir_all(old.join("runtime")).unwrap();
    fs::create_dir(&unknown).unwrap();
    fs::create_dir(&outside).unwrap();
    fs::write(old.join("runtime/workerd.lock.json"), b"{}").unwrap();
    fs::write(old.join("workerd"), b"tool").unwrap();
    fs::write(outside.join("important"), b"keep").unwrap();
    symlink(&outside, packages.join("cd".repeat(32))).unwrap();

    let preview = clean_embedded_runtime_cache(&cache, true).unwrap();
    assert_eq!(preview.entries, 1);
    assert_eq!(preview.bytes, 6);
    assert!(preview.skipped >= 3);
    assert!(old.exists());
    let removed = clean_embedded_runtime_cache(&cache, false).unwrap();
    assert_eq!(removed.entries, 1);
    assert_eq!(removed.bytes, 6);
    assert!(!old.exists());
    assert!(current.exists());
    assert!(unknown.exists());
    assert_eq!(fs::read(outside.join("important")).unwrap(), b"keep");
}
