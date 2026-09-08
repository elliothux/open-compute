use super::*;

#[test]
fn remove_rejects_live_control_socket() {
    let temp = TempDir::new().unwrap();
    let registry = scratch_registry(&temp);
    let (canonical, record) = register_config(&temp, &registry);
    let id = InstanceId::from_canonical_config_path(&canonical).unwrap();
    let runtime_parent = std::env::temp_dir().join(format!("oc-rm-{}", id.as_str()));
    let _ = fs::remove_dir_all(&runtime_parent);
    let runtime = runtime_parent.join(id.as_str());
    fs::create_dir_all(&runtime_parent).unwrap();
    let mut control = publish_ready_control(&runtime, &id, &canonical);
    let selector: InstanceSelector = record.instance_id.parse().unwrap();
    let fake = FakeServiceManager::default();
    let runtime_root = runtime_parent.clone();
    let err = {
        let handle = std::thread::spawn(move || {
            remove_instance(
                &selector,
                &registry,
                &fake,
                Some(runtime_root.as_path()),
                &mut Vec::new(),
            )
        });
        for _ in 0..80 {
            control.poll_once().unwrap();
            if handle.is_finished() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        handle.join().unwrap().unwrap_err()
    };
    assert_eq!(err.code(), ErrorCode::DataDirInUse);
    drop(control);
    let _ = fs::remove_dir_all(&runtime_parent);
}
