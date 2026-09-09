use super::*;

#[test]
fn atomic_replace_and_temp_cleanup() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = tmp.path().join("d");
    fs::create_dir(&dir).unwrap();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
    let dest = dir.join("file");
    atomic_write(&dest, b"one").expect("write1");
    atomic_write(&dest, b"two").expect("write2");
    assert_eq!(fs::read(&dest).unwrap(), b"two");
    let leftovers: Vec<_> = fs::read_dir(&dir)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|e| e.file_name().to_string_lossy().starts_with(".tmp-"))
        .collect();
    assert!(leftovers.is_empty());
}
