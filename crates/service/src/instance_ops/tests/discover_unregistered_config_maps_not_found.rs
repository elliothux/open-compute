use super::*;

#[test]
fn unregistered_config_is_not_discovered() {
    let temp = TempDir::new().unwrap();
    let registry = scratch_registry(&temp);
    let _ = write_loadable_config(temp.path());
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
    assert!(err.message().contains("no running instance"));
}
