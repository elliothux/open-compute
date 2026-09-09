use super::*;

#[test]
fn discover_unregistered_config_maps_not_found() {
    let temp = TempDir::new().unwrap();
    let registry = scratch_registry(&temp);
    let _ = write_loadable_config(temp.path());
    let err = resolve_online_instance(
        None,
        None,
        temp.path(),
        &registry,
        Some(temp.path().join("empty-rt").as_path()),
    )
    .unwrap_err();
    assert_eq!(err.code(), ErrorCode::InstanceNotFound);
    assert!(err.message().contains("not registered"));
}
