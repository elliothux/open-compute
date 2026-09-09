use super::*;

#[test]
fn filesystem_helpers_cover_secure_success_and_failure_paths() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("root");
    fs::create_dir(&root).unwrap();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();

    assert!(sfs::require_absolute(&root).is_ok());
    assert!(sfs::require_absolute(Path::new("relative")).is_err());
    assert!(sfs::require_absolute(Path::new("/tmp/../escape")).is_err());
    assert!(sfs::validate_root(&root).is_ok());
    assert!(sfs::validate_root(&root.join("missing")).is_err());

    let owned_dir = root.join("owned");
    sfs::create_dir_secure(&owned_dir).unwrap();
    sfs::create_dir_secure(&owned_dir).unwrap();
    assert!(sfs::validate_owned_dir(&owned_dir).is_ok());
    let file = root.join("authority");
    sfs::ensure_file_secure(&file).unwrap();
    sfs::ensure_file_secure(&file).unwrap();
    assert!(sfs::validate_owned_file(&file, true).is_ok());
    assert!(sfs::validate_owned_dir(&file).is_err());
    assert!(sfs::validate_owned_file(&owned_dir, true).is_err());

    let loose = root.join("loose");
    fs::write(&loose, b"x").unwrap();
    fs::set_permissions(&loose, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(sfs::validate_owned_file(&loose, false).is_ok());
    assert!(sfs::validate_owned_file(&loose, true).is_err());
    sfs::chmod(&loose, 0o600).unwrap();
    assert!(sfs::validate_owned_file(&loose, true).is_ok());
    assert!(sfs::chmod(&root.join("missing"), 0o600).is_err());

    let link = root.join("link");
    std::os::unix::fs::symlink(&file, &link).unwrap();
    assert!(sfs::validate_owned_file(&link, true).is_err());
    assert!(sfs::open_nofollow(&link, false, false).is_err());
    assert!(sfs::validate_contained(&root, &link).is_err());

    let opened = sfs::open_nofollow(&file, false, false).unwrap();
    sfs::validate_authority_fd(&opened).unwrap();
    let directory_fd = File::open(&owned_dir).unwrap();
    assert!(sfs::validate_authority_fd(&directory_fd).is_err());
    let loose_fd = File::open(&loose).unwrap();
    sfs::validate_authority_fd(&loose_fd).unwrap();
    assert!(sfs::open_nofollow(Path::new("relative"), false, false).is_err());
    assert!(sfs::open_nofollow(&root.join("does-not-exist"), false, false).is_err());
    let created = root.join("created");
    drop(sfs::open_nofollow(&created, true, true).unwrap());
    assert!(created.is_file());

    assert!(sfs::validate_contained(&root, &file).is_ok());
    assert!(sfs::validate_contained(&root, &root.join("future")).is_ok());
    assert!(sfs::validate_contained(&root, tmp.path()).is_err());
    assert!(sfs::validate_contained(&root, &root.join("missing-parent/future")).is_err());
    assert!(sfs::inspect(&root.join("missing")).is_err());
    sfs::fsync_dir(&root).unwrap();
    assert!(sfs::fsync_dir(&root.join("missing")).is_err());

    let nested = tmp.path().join("new-root");
    sfs::create_root_first_run(&nested).unwrap();
    assert!(sfs::create_root_first_run(&nested).is_err());
    assert!(sfs::create_root_first_run(&tmp.path().join("missing-parent/root")).is_err());
    let root_file = tmp.path().join("root-file");
    fs::write(&root_file, b"x").unwrap();
    assert!(sfs::validate_root(&root_file).is_err());
    let root_link = tmp.path().join("root-link");
    std::os::unix::fs::symlink(&root, &root_link).unwrap();
    assert!(sfs::validate_root(&root_link).is_err());

    let atomic = root.join("atomic");
    atomic_write(&atomic, b"one").unwrap();
    atomic_write(&atomic, b"two").unwrap();
    assert_eq!(fs::read(&atomic).unwrap(), b"two");
    assert!(atomic_write(Path::new("relative"), b"x").is_err());
    assert!(atomic_write(&root.join("missing-parent/value"), b"x").is_err());
}
