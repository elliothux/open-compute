use super::*;

#[test]
fn wal_observation_accepts_owned_sidecar_and_rejects_symlink() {
    let fixture = fixture();
    fixture
        .engine
        .exec("CREATE TABLE wal_probe(value INTEGER)", limits())
        .unwrap();
    assert!(fixture.engine.wal_bytes().unwrap() <= fixture.engine.quota_bytes);

    fixture.engine.checkpoint(true).unwrap();
    let mut wal_name = fixture.engine.path.as_os_str().to_os_string();
    wal_name.push("-wal");
    let wal = std::path::PathBuf::from(wal_name);
    if wal.exists() {
        std::fs::remove_file(&wal).unwrap();
    }
    std::os::unix::fs::symlink(&fixture.engine.path, &wal).unwrap();
    assert_eq!(
        fixture.engine.wal_bytes().unwrap_err().code(),
        ErrorCode::D1IdentityMismatch
    );
}
