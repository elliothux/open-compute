use super::*;

#[test]
fn cache_open_rejects_symlink_root() {
    let tmp = TempDir::new().unwrap();
    let real = tmp.path().join("real-root");
    fs::create_dir(&real).unwrap();
    let link = tmp.path().join("link-root");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let err = ArtifactCache::open(link, cache_config(1024), StartupId::generate()).unwrap_err();
    assert_eq!(err.code(), ErrorCode::PathInvalid);
    assert!(real.exists());
    assert!(fs::read_dir(&real).unwrap().next().is_none());
}
