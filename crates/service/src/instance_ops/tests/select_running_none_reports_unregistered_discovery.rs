use super::*;

#[test]
fn select_running_none_reports_unregistered_discovery() {
    let temp = TempDir::new().unwrap();
    let registry = scratch_registry(&temp);
    let err = resolve_online_instance(
        None,
        None,
        temp.path(),
        &registry,
        Some(temp.path().join("empty-rt").as_path()),
    )
    .unwrap_err();
    // Either discovery fails (no config) or discovered config is unregistered.
    assert!(matches!(
        err.code(),
        ErrorCode::InstanceNotFound | ErrorCode::ConfigPathInvalid | ErrorCode::ConfigInvalid
    ));
}
