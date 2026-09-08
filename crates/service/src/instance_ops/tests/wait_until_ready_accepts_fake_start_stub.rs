use super::*;

#[test]
fn wait_until_ready_accepts_fake_start_stub() {
    let temp = TempDir::new().unwrap();
    let registry = scratch_registry(&temp);
    let (_, record) = register_config(&temp, &registry);
    let fake = FakeServiceManager::default();
    let runtime_root = temp.path().join("ready-rt");
    fake.set_ready_runtime_root(Some(runtime_root.clone()));
    fake.start(&record).unwrap();
    wait_until_instance_ready(
        &record,
        Some(runtime_root.as_path()),
        Duration::from_secs(2),
    )
    .unwrap();
    wait_until_instance_ready_for_release(
        &record,
        Some(runtime_root.as_path()),
        Duration::from_secs(2),
        Some(env!("CARGO_PKG_VERSION")),
    )
    .unwrap();
    assert!(
        wait_until_instance_ready_for_release(
            &record,
            Some(runtime_root.as_path()),
            Duration::from_millis(150),
            Some("99.99.99"),
        )
        .is_err()
    );
}
