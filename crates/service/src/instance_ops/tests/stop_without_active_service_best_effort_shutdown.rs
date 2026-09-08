use super::*;

#[test]
fn stop_without_active_service_best_effort_shutdown() {
    let temp = TempDir::new().unwrap();
    let registry = scratch_registry(&temp);
    let (_, record) = register_config(&temp, &registry);
    let selector: InstanceSelector = record.instance_id.parse().unwrap();
    let fake = FakeServiceManager::default();
    let runtime_root = temp.path().join("rt");
    let mut out = Vec::new();
    stop_instance(
        None,
        Some(&selector),
        temp.path(),
        &registry,
        &fake,
        Some(runtime_root.as_path()),
        &mut out,
    )
    .unwrap();
    assert!(String::from_utf8(out).unwrap().contains("INSTANCE_STOPPED"));
}
