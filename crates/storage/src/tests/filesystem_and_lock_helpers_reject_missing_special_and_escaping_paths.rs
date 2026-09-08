use super::*;

#[test]
fn filesystem_and_lock_helpers_reject_missing_special_and_escaping_paths() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("root");
    fs::create_dir(&root).unwrap();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();

    let parent_file = root.join("parent-file");
    fs::write(&parent_file, b"x").unwrap();
    assert_eq!(
        sfs::create_dir_secure(&parent_file.join("child"))
            .unwrap_err()
            .code(),
        ErrorCode::PathInvalid
    );
    assert_eq!(
        sfs::create_root_first_run(Path::new("/"))
            .unwrap_err()
            .code(),
        ErrorCode::PathInvalid
    );
    assert_eq!(
        sfs::create_root_first_run(&root.join("missing/child"))
            .unwrap_err()
            .code(),
        ErrorCode::PathInvalid
    );
    assert_eq!(
        sfs::validate_contained(&root.join("missing"), &root.join("missing/child"))
            .unwrap_err()
            .code(),
        ErrorCode::PathInvalid
    );
    let outside = tmp.path().join("outside");
    fs::write(&outside, b"outside").unwrap();
    assert_eq!(
        sfs::validate_contained(&root, &outside).unwrap_err().code(),
        ErrorCode::PathInvalid
    );
    assert_eq!(
        atomic_write(Path::new("/"), b"x").unwrap_err().code(),
        ErrorCode::PathInvalid
    );
    assert_eq!(
        sfs::chmod(&root.join("missing"), 0o600).unwrap_err().code(),
        ErrorCode::PathInvalid
    );

    assert_eq!(
        crate::DataDirLock::classify_path(&root.join("missing")),
        crate::FilesystemDurability::Unclassified
    );
    assert!(
        crate::FilesystemDurability::ApparentlyLocal
            .doctor_warning()
            .is_none()
    );
    assert!(
        crate::FilesystemDurability::NetworkOrRemote
            .doctor_warning()
            .unwrap()
            .contains("network")
    );
    assert!(
        crate::FilesystemDurability::Unclassified
            .doctor_warning()
            .unwrap()
            .contains("could not be classified")
    );
    assert_eq!(
        crate::InspectLock::try_acquire(&root.join("missing.lock"))
            .unwrap_err()
            .code(),
        ErrorCode::PathInvalid
    );
}
