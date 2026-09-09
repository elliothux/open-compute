use super::*;

#[test]
fn open_dashboard_human_output_without_json() {
    let temp = TempDir::new().unwrap();
    let registry = scratch_registry(&temp);
    let (canonical, record) = register_config(&temp, &registry);
    let id = InstanceId::from_canonical_config_path(&canonical).unwrap();
    let runtime_parent = std::env::temp_dir().join(format!("oc-ops-h-{}", id.as_str()));
    let _ = fs::remove_dir_all(&runtime_parent);
    let runtime = runtime_parent.join(id.as_str());
    fs::create_dir_all(&runtime_parent).unwrap();
    let (tx, _rx) = tokio::sync::watch::channel(false);
    let auth = Arc::new(crate::dashboard_auth::DashboardAuth::new(
        StartupId::generate(),
    ));
    let descriptor = build_descriptor(
        &id,
        &canonical,
        StartupId::generate(),
        PlatformId::generate(),
        "0123456789abcdef0123456789abcdef".to_owned(),
        "0.1.1",
        ServiceScope::User,
        Some("127.0.0.1:8787".to_owned()),
        None,
        "ready",
        SystemTime::UNIX_EPOCH,
    )
    .unwrap();
    let mut control = InstanceControl::publish(&runtime, descriptor, tx, auth).unwrap();
    let selector: InstanceSelector = record.instance_id.parse().unwrap();
    let runtime_root = runtime_parent.clone();
    let issued = std::thread::spawn(move || {
        let mut out = Vec::new();
        open_dashboard(
            None,
            Some(&selector),
            temp.path(),
            &registry,
            Some(runtime_root.as_path()),
            true,
            false,
            &mut out,
        )
        .map(|_| out)
    });
    for _ in 0..100 {
        control.poll_once().unwrap();
        if issued.is_finished() {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let text = String::from_utf8(issued.join().unwrap().unwrap()).unwrap();
    assert!(text.contains("DASHBOARD_URL http://127.0.0.1:8787/operator/#login="));
    assert!(text.contains("LOGIN_EXPIRES_AT_MS "));
    drop(control);
    let _ = fs::remove_dir_all(&runtime_parent);
}
