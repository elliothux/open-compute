use super::*;

#[test]
fn local_root_rejects_symlinks_and_insecure_existing_permissions() {
    let temp = tempfile::tempdir().unwrap();
    let outside = temp.path().join("outside");
    fs::create_dir(&outside).unwrap();
    fs::set_permissions(&outside, fs::Permissions::from_mode(0o700)).unwrap();
    let link = temp.path().join("link");
    std::os::unix::fs::symlink(&outside, &link).unwrap();
    let config = LocalObjectStorageConfig {
        path: link,
        free_space_soft_bytes: 1,
        free_space_hard_bytes: 1,
        ..LocalObjectStorageConfig::default()
    };
    assert!(ObjectBackend::open_local(&config, PlatformId::generate(), LIMIT).is_err());

    let insecure = temp.path().join("insecure");
    fs::create_dir(&insecure).unwrap();
    fs::set_permissions(&insecure, fs::Permissions::from_mode(0o755)).unwrap();
    let config = LocalObjectStorageConfig {
        path: insecure,
        free_space_soft_bytes: 1,
        free_space_hard_bytes: 1,
        ..LocalObjectStorageConfig::default()
    };
    assert!(ObjectBackend::open_local(&config, PlatformId::generate(), LIMIT).is_err());
}
