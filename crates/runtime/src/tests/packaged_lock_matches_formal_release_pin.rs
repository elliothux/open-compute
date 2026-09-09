use super::*;

#[test]
fn packaged_lock_matches_formal_release_pin() {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../packages/runtime/workerd.lock.json");
    let (lock, bytes) = load_runtime_lock(&path.canonicalize().unwrap()).unwrap();
    let (embedded, embedded_bytes) = crate::embedded_runtime_lock().unwrap();
    assert_eq!(lock, embedded);
    assert_eq!(bytes, embedded_bytes);
    assert_eq!(
        lock.targets.keys().map(String::as_str).collect::<Vec<_>>(),
        ["darwin-arm64", "darwin-x64", "linux-arm64", "linux-x64"]
    );
}
