use super::*;

#[test]
fn start_rejects_corrupt_registry_digest() {
    let temp = TempDir::new().unwrap();
    let registry = scratch_registry(&temp);
    let (canonical, record) = register_config(&temp, &registry);
    let path = registry
        .root_for(ServiceScope::User)
        .join(format!("{}.json", record.instance_id));
    let mut body: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    body["digest_sha256"] = serde_json::Value::String("zz".repeat(32));
    fs::write(&path, serde_json::to_vec(&body).unwrap()).unwrap();
    let fake = FakeServiceManager::default();
    let err = start_instance(
        Some(canonical.as_path()),
        None,
        temp.path(),
        &registry,
        &fake,
        &mut Vec::new(),
    )
    .unwrap_err();
    assert_eq!(err.code(), ErrorCode::InstanceRegistryInvalid);
}
