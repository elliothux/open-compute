use super::*;

#[test]
fn select_running_none_does_not_discover_config() {
    let temp = TempDir::new().unwrap();
    let registry = scratch_registry(&temp);
    let err = resolve_online_instance(
        None,
        None,
        temp.path(),
        &registry,
        ServiceScope::User,
        Some(temp.path().join("empty-rt").as_path()),
    )
    .unwrap_err();
    assert_eq!(err.code(), ErrorCode::InstanceNotFound);
}
