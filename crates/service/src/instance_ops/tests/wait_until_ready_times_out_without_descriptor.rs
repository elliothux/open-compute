use super::*;

#[test]
fn wait_until_ready_times_out_without_descriptor() {
    let temp = TempDir::new().unwrap();
    let registry = scratch_registry(&temp);
    let (_, record) = register_config(&temp, &registry);
    let err = wait_until_instance_ready(
        &record,
        Some(temp.path().join("empty-rt").as_path()),
        Duration::from_millis(150),
    )
    .unwrap_err();
    assert_eq!(err.code(), ErrorCode::PlatformUnavailable);
    assert!(err.message().contains("ready"));
}
