use super::*;

#[test]
fn select_running_returns_single_and_rejects_ambiguous() {
    let temp = TempDir::new().unwrap();
    let registry = scratch_registry(&temp);
    let (c1, r1) = register_config(&temp, &registry);
    let dir2 = temp.path().join("b");
    fs::create_dir_all(&dir2).unwrap();
    let c2 = write_loadable_config(&dir2).canonicalize().unwrap();
    let r2 = registry
        .register(&c2, ServiceScope::User, SystemTime::now())
        .unwrap();
    let id1 = InstanceId::from_canonical_config_path(&c1).unwrap();
    let id2 = InstanceId::from_canonical_config_path(&c2).unwrap();
    // Keep sockaddr_un paths short on macOS.
    let runtime_parent = std::env::temp_dir().join(format!("ocs{}", id1.as_str()));
    let _ = fs::remove_dir_all(&runtime_parent);
    fs::create_dir_all(&runtime_parent).unwrap();
    let rt1 = runtime_parent.join(id1.as_str());
    let rt2 = runtime_parent.join(id2.as_str());
    let mut control1 = publish_ready_control(&rt1, &id1, &c1);

    let selected = {
        let registry = registry.clone();
        let runtime_parent = runtime_parent.clone();
        let cwd = temp.path().to_path_buf();
        let handle = std::thread::spawn(move || {
            resolve_online_instance(None, None, &cwd, &registry, Some(runtime_parent.as_path()))
        });
        for _ in 0..50 {
            control1.poll_once().unwrap();
            if handle.is_finished() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        handle.join().unwrap().unwrap()
    };
    assert_eq!(selected.instance_id, r1.instance_id);

    let mut control2 = publish_ready_control(&rt2, &id2, &c2);
    let err = {
        let registry = registry.clone();
        let runtime_parent = runtime_parent.clone();
        let cwd = temp.path().to_path_buf();
        let handle = std::thread::spawn(move || {
            resolve_online_instance(None, None, &cwd, &registry, Some(runtime_parent.as_path()))
        });
        for _ in 0..80 {
            control1.poll_once().unwrap();
            control2.poll_once().unwrap();
            if handle.is_finished() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        handle.join().unwrap().unwrap_err()
    };
    assert_eq!(err.code(), ErrorCode::InstanceAmbiguous);
    let _ = r2;
    drop(control1);
    drop(control2);
    let _ = fs::remove_dir_all(&runtime_parent);
}
