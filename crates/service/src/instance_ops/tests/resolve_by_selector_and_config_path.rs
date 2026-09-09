use super::*;

#[test]
fn resolve_by_selector_and_config_path() {
    let temp = TempDir::new().unwrap();
    let registry = scratch_registry(&temp);
    let (canonical, record) = register_config(&temp, &registry);
    let selector: InstanceSelector = record.instance_id.parse().unwrap();
    assert_eq!(
        resolve_online_instance(None, Some(&selector), temp.path(), &registry, None)
            .unwrap()
            .instance_id,
        record.instance_id
    );
    assert_eq!(
        resolve_online_instance(Some(&canonical), None, temp.path(), &registry, None)
            .unwrap()
            .instance_id,
        record.instance_id
    );
}
