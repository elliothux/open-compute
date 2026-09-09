use super::*;

#[test]
fn remove_rejects_when_manager_reports_active() {
    let temp = TempDir::new().unwrap();
    let registry = scratch_registry(&temp);
    let (_, record) = register_config(&temp, &registry);
    let selector: InstanceSelector = record.instance_id.parse().unwrap();
    let fake = FakeServiceManager::default();
    fake.install(&record, Path::new("/usr/local/bin/ocd"))
        .unwrap();
    fake.start(&record).unwrap();
    let err = remove_instance(
        &selector,
        &registry,
        &fake,
        Some(temp.path().join("empty-rt").as_path()),
        &mut Vec::new(),
    )
    .unwrap_err();
    assert_eq!(err.code(), ErrorCode::DataDirInUse);
    assert!(err.message().contains("still running"));
}
