use super::*;

#[test]
fn start_by_selector_preserves_registered_system_identity() {
    let temp = TempDir::new().unwrap();
    let registry = scratch_registry(&temp);
    let config = write_loadable_config(temp.path()).canonicalize().unwrap();
    let record = registry
        .register_with_service_user(
            &config,
            ServiceScope::System,
            Some("ocd-service"),
            SystemTime::now(),
        )
        .unwrap();
    let selector: InstanceSelector = record.instance_id.parse().unwrap();
    let fake = FakeServiceManager::default();
    start_instance(
        None,
        Some(&selector),
        temp.path(),
        &registry,
        &fake,
        &mut Vec::new(),
    )
    .unwrap();
    let records = registry.list().unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].service_scope, ServiceScope::System);
    assert_eq!(records[0].service_user.as_deref(), Some("ocd-service"));
}
