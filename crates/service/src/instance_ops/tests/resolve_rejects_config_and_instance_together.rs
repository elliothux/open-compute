use super::*;

#[test]
fn resolve_rejects_config_and_instance_together() {
    let temp = TempDir::new().unwrap();
    let registry = scratch_registry(&temp);
    let selector: InstanceSelector = InstanceId::generate().to_string().parse().unwrap();
    let err = resolve_online_instance(
        Some(Path::new("/tmp/x.toml")),
        Some(&selector),
        temp.path(),
        &registry,
        ServiceScope::User,
        None,
    )
    .unwrap_err();
    assert_eq!(err.code(), ErrorCode::ConfigPathInvalid);
}
