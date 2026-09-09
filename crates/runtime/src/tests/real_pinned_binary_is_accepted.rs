use super::*;

#[tokio::test]
async fn real_pinned_binary_is_accepted() {
    let path = std::env::var_os("OPEN_COMPUTE_TEST_WORKERD")
        .expect("OPEN_COMPUTE_TEST_WORKERD must name the verified stock runtime");
    let path = PathBuf::from(path);
    assert!(path.is_absolute());
    let lock_path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../packages/runtime/workerd.lock.json");
    let verified = verify_runtime_binary(
        &lock_path.canonicalize().unwrap(),
        &path,
        Duration::from_secs(10),
        &Redactor::new(),
    )
    .await
    .expect("real workerd must verify");
    let (lock, _) = load_runtime_lock(&lock_path.canonicalize().unwrap()).unwrap();
    assert_eq!(verified.version_output(), lock.expected_version_output);
    assert_eq!(verified.release(), lock.release);
}
