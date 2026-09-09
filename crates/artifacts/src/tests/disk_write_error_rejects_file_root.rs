use super::*;

#[tokio::test]
async fn disk_write_error_rejects_file_root() {
    let tmp = TempDir::new().unwrap();
    let file_root = tmp.path().join("not-a-dir");
    fs::write(&file_root, b"nope").unwrap();
    let err = ArtifactCache::open(file_root, cache_config(1024), StartupId::generate());
    assert_eq!(err.unwrap_err().code(), ErrorCode::PathInvalid);
}
