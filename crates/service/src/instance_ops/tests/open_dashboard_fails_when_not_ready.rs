use super::*;

#[test]
fn open_dashboard_fails_when_not_ready() {
    let temp = TempDir::new().unwrap();
    let registry = scratch_registry(&temp);
    let (_, record) = register_config(&temp, &registry);
    let selector: InstanceSelector = record.instance_id.parse().unwrap();
    let err = open_dashboard(
        None,
        Some(&selector),
        temp.path(),
        &registry,
        Some(temp.path().join("empty-runtime").as_path()),
        true,
        false,
        &mut Vec::new(),
    )
    .unwrap_err();
    assert_eq!(err.code(), ErrorCode::InstanceNotFound);
}
