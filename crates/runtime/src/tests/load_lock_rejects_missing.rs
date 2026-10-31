use super::*;

#[test]
fn load_lock_rejects_missing() {
    let dir = TempDir::new().unwrap();
    let path = write_lock(dir.path(), &"ab".repeat(32));
    load_runtime_lock(&path).expect("regular lock");

    assert_eq!(
        load_runtime_lock(&dir.path().join("missing.json"))
            .unwrap_err()
            .code(),
        ErrorCode::PathInvalid
    );
}
