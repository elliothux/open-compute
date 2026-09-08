use super::*;

#[test]
fn cache_open_rejects_relative_symlink_ancestor_without_mutating_outside() {
    let tmp = TempDir::new().unwrap();
    let base = tmp.path().join("base");
    fs::create_dir(&base).unwrap();
    let outside = base.join("outside");
    fs::create_dir(&outside).unwrap();
    let sentinel = outside.join("sentinel");
    write_mode(&sentinel, "do-not-touch", 0o640);
    fs::set_permissions(&outside, fs::Permissions::from_mode(0o755)).unwrap();
    let outside_mode = fs::metadata(&outside).unwrap().permissions().mode();
    let sentinel_mode = fs::metadata(&sentinel).unwrap().permissions().mode();
    let sentinel_bytes = fs::read(&sentinel).unwrap();

    let link = base.join("link");
    std::os::unix::fs::symlink(Path::new("outside"), &link).unwrap();
    let cache_root = link.join("cache");

    let err =
        ArtifactCache::open(cache_root, cache_config(1024), StartupId::generate()).unwrap_err();
    assert_eq!(err.code(), ErrorCode::PathInvalid);
    assert!(!outside.join("cache").exists());
    assert!(!outside.join("sha256").exists());
    assert_eq!(
        fs::metadata(&outside).unwrap().permissions().mode(),
        outside_mode
    );
    assert_eq!(fs::read(&sentinel).unwrap(), sentinel_bytes);
    assert_eq!(
        fs::metadata(&sentinel).unwrap().permissions().mode(),
        sentinel_mode
    );
}
