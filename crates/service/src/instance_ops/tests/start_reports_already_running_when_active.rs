use super::*;

#[test]
fn start_reports_already_running_when_active() {
    let temp = TempDir::new().unwrap();
    let registry = scratch_registry(&temp);
    let (canonical, record) = register_config(&temp, &registry);
    let fake = FakeServiceManager::default();
    fake.install(&record, Path::new("/usr/local/bin/ocd"))
        .unwrap();
    fake.start(&record).unwrap();
    let mut out = Vec::new();
    start_instance(
        Some(canonical.as_path()),
        None,
        temp.path(),
        &registry,
        &fake,
        &mut out,
    )
    .unwrap();
    assert!(String::from_utf8(out).unwrap().contains("already running"));
}
