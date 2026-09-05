use super::*;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

fn write_mode(path: &Path, bytes: &[u8], mode: u32) {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(mode)
        .open(path)
        .unwrap();
    file.write_all(bytes).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
}

#[test]
fn path_open_mode_hash_and_remove_helpers_cover_failure_boundaries() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    assert!(require_absolute(Path::new("relative")).is_err());
    assert!(require_absolute(&root.join("a/../b")).is_err());
    assert!(
        open_optional_nofollow(&root.join("missing"))
            .unwrap()
            .is_none()
    );

    let file = root.join("file");
    write_mode(&file, b"payload", 0o600);
    assert!(open_optional_nofollow(&file).unwrap().is_some());
    assert!(require_regular_file(&file).is_ok());
    assert_eq!(read_regular_nofollow(&file).unwrap(), b"payload");
    assert!(read_regular_nofollow_bounded(&file, 2).is_err());
    assert!(require_executable_fd(&File::open(&file).unwrap()).is_err());

    fs::set_permissions(&file, fs::Permissions::from_mode(0o722)).unwrap();
    assert!(require_regular_file(&file).is_err());
    assert!(require_executable_fd(&File::open(&file).unwrap()).is_err());
    fs::set_permissions(&file, fs::Permissions::from_mode(0o700)).unwrap();
    assert!(require_executable_fd(&File::open(&file).unwrap()).is_ok());
    let mut opened = File::open(&file).unwrap();
    assert_eq!(hash_file(&mut opened).unwrap(), hash_bytes(b"payload"));
    assert_eq!(
        parse_sha256_hex(&hex_sha256(&hash_bytes(b"payload"))).unwrap(),
        hash_bytes(b"payload")
    );
    assert!(parse_sha256_hex(&"A".repeat(64)).is_err());

    let link = root.join("link");
    std::os::unix::fs::symlink(&file, &link).unwrap();
    assert!(open_optional_nofollow(&link).is_err());
    assert!(require_not_symlink(&link).is_err());
    assert!(reject_symlink_escape(root, &link).is_err());

    let directory = root.join("directory");
    fs::create_dir(&directory).unwrap();
    assert!(require_regular_file(&directory).is_err());
    assert!(require_executable_fd(&File::open(&directory).unwrap()).is_err());
    assert!(remove_file_strict(&directory).is_err());
    assert!(remove_file_nofollow(&directory).is_err());
    assert!(remove_empty_dir_nofollow(&directory).is_ok());
    assert!(remove_empty_dir_nofollow(&directory).is_ok());
    assert!(remove_file_strict(Path::new("relative/missing")).is_ok());
}

#[test]
fn directory_walk_atomic_write_and_workspace_lifecycle_are_complete() {
    // Unix socket fixtures need a short path even under a deeply nested worktree.
    let tmp = tempfile::tempdir_in("/tmp").unwrap();
    let root = tmp.path();
    let data = root.join("data");
    create_dir_secure(&data).unwrap();
    assert_eq!(
        fs::metadata(&data).unwrap().permissions().mode() & 0o777,
        0o700
    );
    create_dir_secure(&data).unwrap();
    chmod(&data, 0o700).unwrap();
    fsync_dir(&data).unwrap();

    let atom = data.join("atom");
    write_atomic_replace(&atom, b"one", FILE_MODE).unwrap();
    write_atomic_replace(&atom, b"two", FILE_MODE).unwrap();
    assert_eq!(fs::read(&atom).unwrap(), b"two");
    assert!(write_atomic_new(&atom, b"three", FILE_MODE).is_err());

    let old = data.join("old");
    let new = data.join("new");
    write_mode(&old, b"old", FILE_MODE);
    write_mode(&new, b"new", FILE_MODE);
    assert!(rename_noreplace(&old, &new).is_err());

    let nested = data.join("nested");
    fs::create_dir(&nested).unwrap();
    write_mode(&nested.join("b"), b"b", FILE_MODE);
    write_mode(&data.join("a"), b"a", FILE_MODE);
    let files = list_files_sorted(&data).unwrap();
    assert!(files.windows(2).all(|pair| pair[0] <= pair[1]));

    let fifo = data.join("socket");
    let socket = std::os::unix::net::UnixListener::bind(&fifo).unwrap();
    assert!(list_files_sorted(&data).is_err());
    drop(socket);
    fs::remove_file(&fifo).unwrap();

    let symlink = data.join("symlink");
    std::os::unix::fs::symlink(&atom, &symlink).unwrap();
    assert!(list_files_sorted(&data).is_err());
    fs::remove_file(&symlink).unwrap();

    let work_path;
    {
        let work = WorkDir::create(&data, ".work").unwrap();
        work_path = work.path().to_owned();
        assert!(work_path.is_dir());
    }
    assert!(!work_path.exists());

    let staging_path;
    {
        let mut staging = StagingDir::create(&data, "stage").unwrap();
        staging_path = staging.path().to_owned();
        staging.persist();
    }
    assert!(staging_path.is_dir());
    fs::remove_dir(staging_path).unwrap();

    assert!(contained_in(&data, root).is_err());
    assert!(contained_in(&data, &root.join("other")).is_err());
}
