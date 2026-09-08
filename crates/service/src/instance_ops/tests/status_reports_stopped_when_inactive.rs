use super::*;

#[test]
fn status_reports_stopped_when_inactive() {
    let temp = TempDir::new().unwrap();
    let registry = scratch_registry(&temp);
    let (_, record) = register_config(&temp, &registry);
    let selector: InstanceSelector = record.instance_id.parse().unwrap();
    let mut out = Vec::new();
    status_instance(
        None,
        Some(&selector),
        temp.path(),
        &registry,
        &FakeServiceManager::default(),
        Some(temp.path().join("rt").as_path()),
        &mut out,
        false,
    )
    .unwrap();
    assert!(String::from_utf8(out).unwrap().contains("stopped"));
}
