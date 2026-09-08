use super::*;
use open_compute_core::InstanceId;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::UNIX_EPOCH;
use uuid::Uuid;

fn scratch() -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    // Unique top-level directory: shared parents under TMPDIR fail Gate cleanup.
    let dir = std::env::temp_dir().join(format!(
        "open-compute-instance-control-{}-{}",
        Uuid::now_v7().as_hyphenated(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn fallback_control_socket_path_fits_macos_limit() {
    let runtime = fallback_user_runtime_root(u32::MAX)
        .join("z".repeat(open_compute_core::INSTANCE_ID_MAX_LEN));
    let socket = runtime.join("control.sock");
    assert!(socket.as_os_str().as_encoded_bytes().len() <= 103);
}

#[test]
fn publish_status_and_shutdown_round_trip() {
    let dir = scratch();
    let config = dir.join("c.toml");
    fs::write(&config, "x=1\n").unwrap();
    let canonical = config.canonicalize().unwrap();
    let id = InstanceId::from_canonical_config_path(&canonical).unwrap();
    // Keep the socket path short for macOS `sockaddr_un` limits.
    let runtime = std::env::temp_dir().join(format!("oc-{}", id.as_str()));
    let _ = fs::remove_dir_all(&runtime);
    let (tx, rx) = tokio::sync::watch::channel(false);
    let descriptor = build_descriptor(
        &id,
        &canonical,
        StartupId::generate(),
        PlatformId::generate(),
        "0.1.1",
        ServiceScope::User,
        Some("127.0.0.1:8787".to_owned()),
        None,
        "ready",
        SystemTime::UNIX_EPOCH,
    )
    .unwrap();
    let mut control = InstanceControl::publish(
        &runtime,
        descriptor.clone(),
        tx,
        Arc::new(DashboardAuth::new(StartupId::generate())),
    )
    .unwrap();
    assert_eq!(
        read_descriptor(&runtime).unwrap().unwrap().instance_id,
        id.as_str()
    );
    let runtime_for_probe = runtime.clone();
    let expected_id = descriptor.instance_id.clone();
    let probe = std::thread::spawn(move || probe_status(&runtime_for_probe));
    // Accept the concurrent status probe.
    for _ in 0..50 {
        control.poll_once().unwrap();
        if probe.is_finished() {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let probed = probe.join().unwrap().unwrap().unwrap();
    assert_eq!(probed.instance_id, expected_id);
    let runtime_for_shutdown = runtime.clone();
    let shutdown = std::thread::spawn(move || request_shutdown(&runtime_for_shutdown));
    for _ in 0..50 {
        control.poll_once().unwrap();
        if shutdown.is_finished() {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    shutdown.join().unwrap().unwrap();
    assert!(*rx.borrow());
    drop(control);
    assert!(!runtime.join("control.sock").exists());
    let _ = fs::remove_dir_all(&runtime);
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn login_code_round_trip_via_control_socket() {
    let dir = scratch();
    let config = dir.join("c.toml");
    fs::write(&config, "x=1\n").unwrap();
    let canonical = config.canonicalize().unwrap();
    let id = InstanceId::from_canonical_config_path(&canonical).unwrap();
    let runtime = std::env::temp_dir().join(format!("oc-login-{}", id.as_str()));
    let _ = fs::remove_dir_all(&runtime);
    let (tx, _rx) = tokio::sync::watch::channel(false);
    let auth = Arc::new(DashboardAuth::new(StartupId::generate()));
    let descriptor = build_descriptor(
        &id,
        &canonical,
        StartupId::generate(),
        PlatformId::generate(),
        "0.1.1",
        ServiceScope::User,
        Some("127.0.0.1:8787".to_owned()),
        None,
        "ready",
        SystemTime::UNIX_EPOCH,
    )
    .unwrap();
    let mut control = InstanceControl::publish(&runtime, descriptor, tx, auth.clone()).unwrap();
    let runtime_for_code = runtime.clone();
    let issued = std::thread::spawn(move || request_login_code(&runtime_for_code));
    // The workspace Gate runs several real-process targets concurrently;
    // keep serving the nonblocking socket until the client has actually
    // received its response instead of assuming it will be scheduled in
    // the first 500 ms.
    for _ in 0..500 {
        control.poll_once().unwrap();
        if issued.is_finished() {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(issued.is_finished(), "login-code client did not complete");
    let (code, _expires) = issued.join().unwrap().unwrap();
    let session = auth.exchange_login_code(&code, SystemTime::now()).unwrap();
    assert!(auth.session_valid(&session.session_token, SystemTime::now()));
    drop(control);
    let _ = fs::remove_dir_all(&runtime);
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn read_descriptor_rejects_symlink_and_bad_schema() {
    let dir = scratch();
    let runtime = dir.join("rt");
    fs::create_dir_all(&runtime).unwrap();
    let target = runtime.join("target.json");
    fs::write(&target, b"{}").unwrap();
    std::os::unix::fs::symlink(&target, runtime.join("descriptor.json")).unwrap();
    let err = read_descriptor(&runtime).unwrap_err();
    assert!(err.message().contains("symlink"));

    fs::remove_file(runtime.join("descriptor.json")).unwrap();
    fs::write(
        runtime.join("descriptor.json"),
        br#"{"schema_version":99,"instance_id":"a","canonical_config_path":"/x","startup_id":"s","platform_id":"p","release_version":"0","service_scope":"user","public_listener":null,"admin_listener":null,"readiness":"ready","published_at":0}"#,
    )
    .unwrap();
    let err = read_descriptor(&runtime).unwrap_err();
    assert!(err.message().contains("schema"));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn probe_status_returns_none_without_socket() {
    let dir = scratch();
    assert!(probe_status(&dir).unwrap().is_none());
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn runtime_dir_override_and_scope_paths() {
    let config = scratch().join("c.toml");
    // scratch() already created the dir; write config there.
    let dir = config.parent().unwrap().to_path_buf();
    fs::write(&config, "x=1\n").unwrap();
    let id = InstanceId::from_canonical_config_path(&config.canonicalize().unwrap()).unwrap();
    let override_root = Path::new("/tmp/oc-runtime-override");
    assert_eq!(
        runtime_dir_for(ServiceScope::User, &id, Some(override_root)),
        override_root.join(id.as_str())
    );
    assert!(runtime_dir_for(ServiceScope::System, &id, None).starts_with("/run/open-compute"));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn update_descriptor_and_debug() {
    let dir = scratch();
    let config = dir.join("c.toml");
    fs::write(&config, "x=1\n").unwrap();
    let canonical = config.canonicalize().unwrap();
    let id = InstanceId::from_canonical_config_path(&canonical).unwrap();
    let runtime = std::env::temp_dir().join(format!("oc-upd-{}", id.as_str()));
    let _ = fs::remove_dir_all(&runtime);
    let (tx, _rx) = tokio::sync::watch::channel(false);
    let mut descriptor = build_descriptor(
        &id,
        &canonical,
        StartupId::generate(),
        PlatformId::generate(),
        "0.1.1",
        ServiceScope::User,
        Some("127.0.0.1:8787".to_owned()),
        None,
        "starting",
        SystemTime::UNIX_EPOCH,
    )
    .unwrap();
    let mut control = InstanceControl::publish(
        &runtime,
        descriptor.clone(),
        tx,
        Arc::new(DashboardAuth::new(StartupId::generate())),
    )
    .unwrap();
    assert_eq!(control.descriptor().readiness, "starting");
    descriptor.readiness = "ready".to_owned();
    control.update_descriptor(descriptor.clone()).unwrap();
    assert_eq!(control.descriptor().readiness, "ready");
    let debug = format!("{control:?}");
    assert!(debug.contains("InstanceControl"));
    drop(control);
    let _ = fs::remove_dir_all(&runtime);
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn invalid_control_json_and_empty_poll_are_fail_closed() {
    let dir = scratch();
    let config = dir.join("c.toml");
    fs::write(&config, "x=1\n").unwrap();
    let canonical = config.canonicalize().unwrap();
    let id = InstanceId::from_canonical_config_path(&canonical).unwrap();
    let runtime = std::env::temp_dir().join(format!("oc-bad-{}", id.as_str()));
    let _ = fs::remove_dir_all(&runtime);
    let (tx, _rx) = tokio::sync::watch::channel(false);
    let descriptor = build_descriptor(
        &id,
        &canonical,
        StartupId::generate(),
        PlatformId::generate(),
        "0.1.1",
        ServiceScope::User,
        None,
        None,
        "ready",
        SystemTime::UNIX_EPOCH,
    )
    .unwrap();
    let mut control = InstanceControl::publish(
        &runtime,
        descriptor,
        tx,
        Arc::new(DashboardAuth::new(StartupId::generate())),
    )
    .unwrap();
    control.poll_once().unwrap(); // WouldBlock idle accept
    let socket = runtime.join("control.sock");
    let runtime_for_bad = runtime.clone();
    let bad = std::thread::spawn(move || {
        let mut stream = UnixStream::connect(runtime_for_bad.join("control.sock")).unwrap();
        writeln!(stream, "not-json").unwrap();
        let mut body = String::new();
        let _ = stream.read_to_string(&mut body);
        body
    });
    for _ in 0..50 {
        control.poll_once().unwrap();
        if bad.is_finished() {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let body = bad.join().unwrap();
    assert!(body.contains("\"ok\":false") || body.contains("CONFIG_INVALID"));
    // Oversize request without newline should stop at the 16KiB bound.
    let runtime_for_big = runtime.clone();
    let big = std::thread::spawn(move || {
        let mut stream = UnixStream::connect(runtime_for_big.join("control.sock")).unwrap();
        let payload = vec![b'x'; 20 * 1024];
        let _ = stream.write_all(&payload);
        let mut body = String::new();
        let _ = stream.read_to_string(&mut body);
        body
    });
    for _ in 0..50 {
        control.poll_once().unwrap();
        if big.is_finished() {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let _ = big.join().unwrap();
    drop(control);
    let _ = socket;
    let _ = fs::remove_dir_all(&runtime);
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn request_shutdown_and_login_fail_without_socket() {
    let dir = scratch();
    assert!(request_shutdown(&dir).is_err());
    assert!(request_login_code(&dir).is_err());
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn read_descriptor_none_and_oversize() {
    let dir = scratch();
    assert!(read_descriptor(&dir).unwrap().is_none());
    let runtime = dir.join("rt");
    fs::create_dir_all(&runtime).unwrap();
    let huge = vec![b'a'; 65 * 1024];
    fs::write(runtime.join("descriptor.json"), huge).unwrap();
    let err = read_descriptor(&runtime).unwrap_err();
    assert!(
        err.message().contains("size")
            || err.message().contains("bound")
            || err.message().contains("descriptor")
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn build_descriptor_rejects_pre_epoch_clock() {
    let dir = scratch();
    let config = dir.join("c.toml");
    fs::write(&config, "x=1\n").unwrap();
    let id = InstanceId::from_canonical_config_path(&config.canonicalize().unwrap()).unwrap();
    let err = build_descriptor(
        &id,
        &config,
        StartupId::generate(),
        PlatformId::generate(),
        "0.1.1",
        ServiceScope::User,
        None,
        None,
        "ready",
        UNIX_EPOCH - Duration::from_secs(1),
    )
    .unwrap_err();
    assert_eq!(err.code(), ErrorCode::InstanceRegistryInvalid);
    let _ = fs::remove_dir_all(dir);
}
