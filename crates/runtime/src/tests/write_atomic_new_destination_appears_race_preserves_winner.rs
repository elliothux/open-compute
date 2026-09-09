use super::*;

#[test]
fn write_atomic_new_destination_appears_race_preserves_winner() {
    let dir = TempDir::new().unwrap();
    let dest = dir.path().join("atom");
    set_publish_hook({
        let dest = dest.clone();
        move |path| {
            if path == dest {
                fs::write(&dest, b"winner-bytes").unwrap();
                let mut p = fs::metadata(&dest).unwrap().permissions();
                p.set_mode(0o600);
                fs::set_permissions(&dest, p).unwrap();
            }
        }
    });
    let err = write_atomic_new(&dest, b"loser-bytes", FILE_MODE).unwrap_err();
    clear_publish_hook();
    assert_eq!(err.code(), ErrorCode::PathInvalid);
    assert_eq!(fs::read(&dest).unwrap(), b"winner-bytes");
    let leftovers: Vec<_> = fs::read_dir(dir.path())
        .unwrap()
        .filter_map(Result::ok)
        .map(|e| e.file_name())
        .collect();
    assert!(
        leftovers
            .iter()
            .all(|n| !n.to_string_lossy().contains("partial")),
        "temp file must be removed: {leftovers:?}"
    );
}
