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

fn socket_dir(prefix: &str) -> PathBuf {
    Path::new("/tmp").join(format!("{prefix}-{}", Uuid::now_v7().as_simple()))
}

#[test]
fn scoped_control_socket_path_is_bounded() {
    let id = InstanceId::generate();
    let runtime = runtime_dir_for(ServiceScope::User, &id, Some(Path::new("/tmp/oc-run"))).unwrap();
    let socket = runtime.join("control.sock");
    assert!(unix_socket_path_is_valid(&socket));
    assert!(!unix_socket_path_is_valid(
        &Path::new("/tmp").join("x".repeat(100))
    ));
}

#[test]
fn publish_status_and_shutdown_round_trip() {
    let dir = scratch();
    let config = dir.join("c.toml");
    fs::write(&config, "x=1\n").unwrap();
    let canonical = config.canonicalize().unwrap();
    let id = InstanceId::generate();
    // Keep the socket path short for macOS `sockaddr_un` limits.
    let runtime = socket_dir("oc");
    let _ = fs::remove_dir_all(&runtime);
    let (tx, rx) = tokio::sync::watch::channel(false);
    let descriptor = build_descriptor(
        &id,
        &canonical,
        StartupId::generate(),
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
    let id = InstanceId::generate();
    let runtime = socket_dir("oc-login");
    let _ = fs::remove_dir_all(&runtime);
    let (tx, _rx) = tokio::sync::watch::channel(false);
    let auth = Arc::new(DashboardAuth::new(StartupId::generate()));
    let descriptor = build_descriptor(
        &id,
        &canonical,
        StartupId::generate(),
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
        br#"{"schema_version":99,"instance_id":"0123456789abcdef0123456789abcdef","canonical_config_path":"/x","startup_id":"s","release_version":"0","service_scope":"user","public_listener":null,"admin_listener":null,"readiness":"ready","published_at":0}"#,
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
    let id = InstanceId::generate();
    let override_root = Path::new("/tmp/oc-runtime-override");
    assert_eq!(
        runtime_dir_for(ServiceScope::User, &id, Some(override_root)).unwrap(),
        override_root.join(id.as_str())
    );
    assert!(
        runtime_dir_for(ServiceScope::System, &id, None)
            .unwrap()
            .starts_with("/var/lib/open-compute/run")
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn update_descriptor_and_debug() {
    let dir = scratch();
    let config = dir.join("c.toml");
    fs::write(&config, "x=1\n").unwrap();
    let canonical = config.canonicalize().unwrap();
    let id = InstanceId::generate();
    let runtime = socket_dir("oc-upd");
    let _ = fs::remove_dir_all(&runtime);
    let (tx, _rx) = tokio::sync::watch::channel(false);
    let mut descriptor = build_descriptor(
        &id,
        &canonical,
        StartupId::generate(),
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
    let id = InstanceId::generate();
    let runtime = socket_dir("oc-bad");
    let _ = fs::remove_dir_all(&runtime);
    let (tx, _rx) = tokio::sync::watch::channel(false);
    let descriptor = build_descriptor(
        &id,
        &canonical,
        StartupId::generate(),
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
    let id = InstanceId::generate();
    let err = build_descriptor(
        &id,
        &config,
        StartupId::generate(),
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

fn fake_control_response(body: &'static str) -> (PathBuf, std::thread::JoinHandle<()>) {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let runtime =
        std::env::temp_dir().join(format!("oc-f{}", COUNTER.fetch_add(1, Ordering::Relaxed)));
    let _ = fs::remove_dir_all(&runtime);
    fs::create_dir(&runtime).unwrap();
    let listener = UnixListener::bind(runtime.join("control.sock")).unwrap();
    let task = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0_u8; 256];
        let _ = stream.read(&mut request);
        writeln!(stream, "{body}").unwrap();
        stream.flush().unwrap();
        std::thread::sleep(Duration::from_millis(10));
    });
    (runtime, task)
}

#[test]
fn control_clients_reject_failed_incomplete_and_malformed_responses() {
    for (body, expected) in [
        (
            r#"{"schema_version":1,"ok":false,"error":"INTERNAL","message":null,"descriptor":null,"login_code":null,"login_expires_at":null}"#,
            ErrorCode::PlatformUnavailable,
        ),
        (
            r#"{"schema_version":1,"ok":true,"error":null,"message":null,"descriptor":null,"login_code":null,"login_expires_at":null}"#,
            ErrorCode::PlatformUnavailable,
        ),
        ("not-json", ErrorCode::InstanceRegistryInvalid),
    ] {
        let (runtime, server) = fake_control_response(body);
        let error = request_login_code(&runtime).unwrap_err();
        assert_eq!(error.code(), expected, "{error:?}");
        server.join().unwrap();
        let _ = fs::remove_dir_all(runtime);
    }

    for (body, expected_none) in [
        ("", true),
        (
            r#"{"schema_version":1,"ok":false,"error":"INTERNAL","message":null,"descriptor":null,"login_code":null,"login_expires_at":null}"#,
            true,
        ),
        ("not-json", false),
    ] {
        let (runtime, server) = fake_control_response(body);
        let result = probe_status(&runtime);
        if expected_none {
            assert!(result.unwrap().is_none());
        } else {
            assert_eq!(
                result.unwrap_err().code(),
                ErrorCode::InstanceRegistryInvalid
            );
        }
        server.join().unwrap();
        let _ = fs::remove_dir_all(runtime);
    }
}
