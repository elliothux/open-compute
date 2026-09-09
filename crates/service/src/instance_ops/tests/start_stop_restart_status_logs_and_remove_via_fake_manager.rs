use super::*;

#[test]
fn start_stop_restart_status_logs_and_remove_via_fake_manager() {
    let temp = TempDir::new().unwrap();
    let registry = scratch_registry(&temp);
    let config = write_loadable_config(temp.path());
    let fake = FakeServiceManager::default();
    let mut out = Vec::new();
    start_instance(Some(&config), None, temp.path(), &registry, &fake, &mut out).unwrap();
    let text = String::from_utf8(out).unwrap();
    assert!(text.contains("INSTANCE_STARTED"));
    assert_eq!(fake.installed().len(), 1);
    assert_eq!(fake.started().len(), 1);

    let listed = registry.list().unwrap();
    assert_eq!(listed.len(), 1);
    let selector: InstanceSelector = listed[0].instance_id.parse().unwrap();
    let runtime = temp.path().join("runtime");

    let mut out = Vec::new();
    status_instance(
        None,
        Some(&selector),
        temp.path(),
        &registry,
        &fake,
        Some(runtime.as_path()),
        &mut out,
        false,
    )
    .unwrap();
    assert!(String::from_utf8(out).unwrap().contains("starting"));

    let mut out = Vec::new();
    status_instance(
        None,
        Some(&selector),
        temp.path(),
        &registry,
        &fake,
        Some(runtime.as_path()),
        &mut out,
        true,
    )
    .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(payload["state"], "starting");
    assert_eq!(payload["command"], "status");

    let mut out = Vec::new();
    logs_instance(
        None,
        Some(&selector),
        temp.path(),
        &registry,
        &fake,
        &mut out,
        false,
    )
    .unwrap();
    assert!(String::from_utf8(out).unwrap().contains("fake logs"));

    let mut out = Vec::new();
    restart_instance(
        None,
        Some(&selector),
        temp.path(),
        &registry,
        &fake,
        &mut out,
    )
    .unwrap();
    assert!(
        String::from_utf8(out)
            .unwrap()
            .contains("INSTANCE_RESTARTED")
    );

    let mut out = Vec::new();
    stop_instance(
        None,
        Some(&selector),
        temp.path(),
        &registry,
        &fake,
        None,
        &mut out,
    )
    .unwrap();
    assert!(String::from_utf8(out).unwrap().contains("INSTANCE_STOPPED"));
    assert!(!fake.is_active(&listed[0]).unwrap());

    fake.start(&listed[0]).unwrap();
    let mut out = Vec::new();
    start_instance(
        None,
        Some(&selector),
        temp.path(),
        &registry,
        &fake,
        &mut out,
    )
    .unwrap();
    assert!(String::from_utf8(out).unwrap().contains("INSTANCE_OK"));

    fake.stop(&listed[0]).unwrap();
    fake.start(&listed[0]).unwrap();
    let err_active =
        remove_instance(&selector, &registry, &fake, None, &mut Vec::new()).unwrap_err();
    assert_eq!(err_active.code(), ErrorCode::DataDirInUse);

    fake.stop(&listed[0]).unwrap();
    let mut out = Vec::new();
    remove_instance(&selector, &registry, &fake, None, &mut out).unwrap();
    assert!(String::from_utf8(out).unwrap().contains("INSTANCE_REMOVED"));
    assert!(registry.list().unwrap().is_empty());
}
