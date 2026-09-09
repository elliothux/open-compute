use super::*;

#[test]
fn start_rejects_mutual_exclusive_flags() {
    let temp = TempDir::new().unwrap();
    let registry = scratch_registry(&temp);
    let selector: InstanceSelector = "abcde".parse().unwrap();
    let err = start_instance(
        Some(Path::new("/tmp/x.toml")),
        Some(&selector),
        temp.path(),
        &registry,
        &FakeServiceManager::default(),
        &mut Vec::new(),
    )
    .unwrap_err();
    assert_eq!(err.code(), ErrorCode::ConfigPathInvalid);
}
