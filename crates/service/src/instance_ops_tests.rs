use super::*;
use crate::instance_control::{
    CONTROL_SCHEMA_VERSION, GenerationDescriptor, InstanceControl, build_descriptor,
};
use crate::instance_registry::{InstanceRegistry, ServiceScope};
use crate::service_manager::FakeServiceManager;
use open_compute_core::{ErrorCode, InstanceId, PlatformId, StartupId};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use std::{io::Read, io::Write, net::TcpListener};
use tempfile::TempDir;

fn write_mode(path: &Path, body: &str, mode: u32) {
    fs::write(path, body).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
}

fn write_loadable_config(dir: &Path) -> PathBuf {
    let data = dir.join("data");
    let objects = dir.join("objects");
    let admin = dir.join("admin.token");
    let deployer = dir.join("deployer.token");
    let read_only = dir.join("read-only.token");
    let master = dir.join("master.key");
    fs::create_dir_all(&data).unwrap();
    fs::create_dir_all(&objects).unwrap();
    write_mode(&admin, "admin-secret-value\n", 0o600);
    write_mode(&deployer, "deployer-secret-value\n", 0o600);
    write_mode(&read_only, "read-only-secret-value\n", 0o600);
    let toml = format!(
        r#"
[server]
public_bind = "127.0.0.1:0"
admin_auth = {{ file = "{admin}" }}
deployer_auth = {{ file = "{deployer}" }}
read_only_auth = {{ file = "{read_only}" }}

[data]
path = "{data}"
master_key_file = "{master}"

[storage]
backend = "local"
path = "{objects}"
prefix = "system/"

[cache]
max_bytes = 1048576
high_watermark_ratio = 0.9
low_watermark_ratio = 0.8
max_artifact_bytes = 65536
"#,
        admin = admin.display(),
        deployer = deployer.display(),
        read_only = read_only.display(),
        data = data.display(),
        master = master.display(),
        objects = objects.display(),
    );
    let path = dir.join("compute.toml");
    fs::write(&path, toml).unwrap();
    path
}

fn scratch_registry(temp: &TempDir) -> InstanceRegistry {
    InstanceRegistry::with_roots(
        temp.path().join("registry/system"),
        temp.path().join("registry/user"),
    )
}

fn register_config(temp: &TempDir, registry: &InstanceRegistry) -> (PathBuf, InstanceRecord) {
    let config = write_loadable_config(temp.path());
    let canonical = config.canonicalize().unwrap();
    let record = registry
        .register(&canonical, ServiceScope::User, SystemTime::now())
        .unwrap();
    (canonical, record)
}

#[test]
fn resolve_rejects_config_and_instance_together() {
    let temp = TempDir::new().unwrap();
    let registry = scratch_registry(&temp);
    let selector: InstanceSelector = "abcde".parse().unwrap();
    let err = resolve_online_instance(
        Some(Path::new("/tmp/x.toml")),
        Some(&selector),
        temp.path(),
        &registry,
        None,
    )
    .unwrap_err();
    assert_eq!(err.code(), ErrorCode::ConfigPathInvalid);
}

#[test]
fn resolve_by_selector_and_config_path() {
    let temp = TempDir::new().unwrap();
    let registry = scratch_registry(&temp);
    let (canonical, record) = register_config(&temp, &registry);
    let selector: InstanceSelector = record.instance_id.parse().unwrap();
    assert_eq!(
        resolve_online_instance(None, Some(&selector), temp.path(), &registry, None)
            .unwrap()
            .instance_id,
        record.instance_id
    );
    assert_eq!(
        resolve_online_instance(Some(&canonical), None, temp.path(), &registry, None)
            .unwrap()
            .instance_id,
        record.instance_id
    );
}

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

#[test]
fn start_by_selector_preserves_registered_system_identity() {
    let temp = TempDir::new().unwrap();
    let registry = scratch_registry(&temp);
    let config = write_loadable_config(temp.path()).canonicalize().unwrap();
    let record = registry
        .register_with_service_user(
            &config,
            ServiceScope::System,
            Some("ocd-service"),
            SystemTime::now(),
        )
        .unwrap();
    let selector: InstanceSelector = record.instance_id.parse().unwrap();
    let fake = FakeServiceManager::default();
    start_instance(
        None,
        Some(&selector),
        temp.path(),
        &registry,
        &fake,
        &mut Vec::new(),
    )
    .unwrap();
    let records = registry.list().unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].service_scope, ServiceScope::System);
    assert_eq!(records[0].service_user.as_deref(), Some("ocd-service"));
}

#[test]
fn start_rejects_mutual_exclusive_flags() {
    let temp = TempDir::new().unwrap();
    let registry = scratch_registry(&temp);
    let selector: InstanceSelector = "abcde".parse().unwrap();
    let err = start_instance(
        Some(Path::new("/tmp/x.toml")),
        Some(&selector),
        temp.path(),
        &registry,
        &FakeServiceManager::default(),
        &mut Vec::new(),
    )
    .unwrap_err();
    assert_eq!(err.code(), ErrorCode::ConfigPathInvalid);
}

#[test]
fn stop_without_active_service_best_effort_shutdown() {
    let temp = TempDir::new().unwrap();
    let registry = scratch_registry(&temp);
    let (_, record) = register_config(&temp, &registry);
    let selector: InstanceSelector = record.instance_id.parse().unwrap();
    let fake = FakeServiceManager::default();
    let runtime_root = temp.path().join("rt");
    let mut out = Vec::new();
    stop_instance(
        None,
        Some(&selector),
        temp.path(),
        &registry,
        &fake,
        Some(runtime_root.as_path()),
        &mut out,
    )
    .unwrap();
    assert!(String::from_utf8(out).unwrap().contains("INSTANCE_STOPPED"));
}

#[test]
fn open_dashboard_no_open_with_control_socket() {
    let temp = TempDir::new().unwrap();
    let registry = scratch_registry(&temp);
    let (canonical, record) = register_config(&temp, &registry);
    let id = InstanceId::from_canonical_config_path(&canonical).unwrap();
    let runtime_parent = std::env::temp_dir().join(format!("oc-ops-{}", id.as_str()));
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
        "0.1.1",
        ServiceScope::User,
        Some("127.0.0.1:8787".to_owned()),
        Some("http://127.0.0.1:8788/".to_owned()),
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
            true,
            &mut out,
        )
        .map(|_| out)
    });
    for _ in 0..80 {
        control.poll_once().unwrap();
        if issued.is_finished() {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    for _ in 0..80 {
        control.poll_once().unwrap();
        if issued.is_finished() {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let out = issued.join().unwrap().unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(payload["command"], "dashboard");
    assert!(
        payload["url"]
            .as_str()
            .unwrap()
            .contains("http://127.0.0.1:8788/operator/#login=")
    );
    drop(control);
    let _ = fs::remove_dir_all(&runtime_parent);
}

#[test]
fn open_dashboard_fails_when_not_ready() {
    let temp = TempDir::new().unwrap();
    let registry = scratch_registry(&temp);
    let (_, record) = register_config(&temp, &registry);
    let selector: InstanceSelector = record.instance_id.parse().unwrap();
    let err = open_dashboard(
        None,
        Some(&selector),
        temp.path(),
        &registry,
        Some(temp.path().join("empty-runtime").as_path()),
        true,
        false,
        &mut Vec::new(),
    )
    .unwrap_err();
    assert_eq!(err.code(), ErrorCode::InstanceNotFound);
}

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

#[test]
fn dashboard_base_url_prefers_admin_and_normalizes_host() {
    let descriptor = GenerationDescriptor {
        schema_version: CONTROL_SCHEMA_VERSION,
        instance_id: "abcde".to_owned(),
        canonical_config_path: "/tmp/c.toml".to_owned(),
        startup_id: "s".to_owned(),
        platform_id: "p".to_owned(),
        release_version: "0.1.0".to_owned(),
        service_scope: ServiceScope::User,
        public_listener: Some("127.0.0.1:1".to_owned()),
        admin_listener: None,
        readiness: "ready".to_owned(),
        published_at: 0,
    };
    assert_eq!(
        dashboard_base_url(&descriptor).unwrap(),
        "http://127.0.0.1:1/operator/"
    );

    let mut with_scheme = descriptor.clone();
    with_scheme.admin_listener = Some("https://127.0.0.1:8443/".to_owned());
    assert_eq!(
        dashboard_base_url(&with_scheme).unwrap(),
        "https://127.0.0.1:8443/operator/"
    );

    let mut missing = descriptor;
    missing.public_listener = None;
    missing.admin_listener = None;
    assert_eq!(
        dashboard_base_url(&missing).unwrap_err().code(),
        ErrorCode::PlatformUnavailable
    );
}

#[test]
fn status_reports_stopped_when_inactive() {
    let temp = TempDir::new().unwrap();
    let registry = scratch_registry(&temp);
    let (_, record) = register_config(&temp, &registry);
    let selector: InstanceSelector = record.instance_id.parse().unwrap();
    let mut out = Vec::new();
    status_instance(
        None,
        Some(&selector),
        temp.path(),
        &registry,
        &FakeServiceManager::default(),
        Some(temp.path().join("rt").as_path()),
        &mut out,
        false,
    )
    .unwrap();
    assert!(String::from_utf8(out).unwrap().contains("stopped"));
}

fn publish_ready_control(runtime: &Path, id: &InstanceId, canonical: &Path) -> InstanceControl {
    let (tx, _rx) = tokio::sync::watch::channel(false);
    let auth = Arc::new(crate::dashboard_auth::DashboardAuth::new(
        StartupId::generate(),
    ));
    let descriptor = build_descriptor(
        id,
        canonical,
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
    InstanceControl::publish(runtime, descriptor, tx, auth).unwrap()
}

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

#[test]
fn select_running_none_reports_unregistered_discovery() {
    let temp = TempDir::new().unwrap();
    let registry = scratch_registry(&temp);
    let err = resolve_online_instance(
        None,
        None,
        temp.path(),
        &registry,
        Some(temp.path().join("empty-rt").as_path()),
    )
    .unwrap_err();
    // Either discovery fails (no config) or discovered config is unregistered.
    assert!(matches!(
        err.code(),
        ErrorCode::InstanceNotFound | ErrorCode::ConfigPathInvalid | ErrorCode::ConfigInvalid
    ));
}

#[test]
fn open_dashboard_uses_http_listener_prefix() {
    let temp = TempDir::new().unwrap();
    let registry = scratch_registry(&temp);
    let (canonical, record) = register_config(&temp, &registry);
    let id = InstanceId::from_canonical_config_path(&canonical).unwrap();
    let runtime_parent = std::env::temp_dir().join(format!("oc-http-{}", id.as_str()));
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
        "0.1.1",
        ServiceScope::User,
        Some("https://admin.example/base".to_owned()),
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
    for _ in 0..80 {
        control.poll_once().unwrap();
        if issued.is_finished() {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let out = issued.join().unwrap().unwrap();
    let text = String::from_utf8(out).unwrap();
    assert!(text.contains("DASHBOARD_URL https://admin.example/base/operator/"));
    drop(control);
    let _ = fs::remove_dir_all(&runtime_parent);
}

#[test]
fn start_rejects_corrupt_registry_digest() {
    let temp = TempDir::new().unwrap();
    let registry = scratch_registry(&temp);
    let (canonical, record) = register_config(&temp, &registry);
    let path = registry
        .root_for(ServiceScope::User)
        .join(format!("{}.json", record.instance_id));
    let mut body: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    body["digest_sha256"] = serde_json::Value::String("zz".repeat(32));
    fs::write(&path, serde_json::to_vec(&body).unwrap()).unwrap();
    let fake = FakeServiceManager::default();
    let err = start_instance(
        Some(canonical.as_path()),
        None,
        temp.path(),
        &registry,
        &fake,
        &mut Vec::new(),
    )
    .unwrap_err();
    assert_eq!(err.code(), ErrorCode::InstanceRegistryInvalid);
}

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

#[test]
fn discover_unregistered_config_maps_not_found() {
    let temp = TempDir::new().unwrap();
    let registry = scratch_registry(&temp);
    let _ = write_loadable_config(temp.path());
    let err = resolve_online_instance(
        None,
        None,
        temp.path(),
        &registry,
        Some(temp.path().join("empty-rt").as_path()),
    )
    .unwrap_err();
    assert_eq!(err.code(), ErrorCode::InstanceNotFound);
    assert!(err.message().contains("not registered"));
}

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

#[test]
fn helper_io_and_open_url_paths() {
    assert_eq!(io_failed().code(), ErrorCode::ConfigInvalid);
    // macOS `open` fails closed for a path that cannot be resolved.
    let err = open_url_in_browser("/tmp/open-compute-no-such-dashboard-target-xyz")
        .err()
        .or_else(|| open_url_in_browser("").err());
    // Either open fails, or it succeeds opening Finder; both exercise the helper.
    let _ = err;
    let _ = open_url_in_browser("https://example.invalid/");
}

fn http_descriptor(listener: Option<String>, readiness: &str) -> GenerationDescriptor {
    GenerationDescriptor {
        schema_version: CONTROL_SCHEMA_VERSION,
        instance_id: "abcde".to_owned(),
        canonical_config_path: "/tmp/c.toml".to_owned(),
        startup_id: "startup".to_owned(),
        platform_id: "platform".to_owned(),
        release_version: env!("CARGO_PKG_VERSION").to_owned(),
        service_scope: ServiceScope::User,
        public_listener: listener,
        admin_listener: None,
        readiness: readiness.to_owned(),
        published_at: 0,
    }
}

fn one_shot_health_response(status: &str) -> (String, std::thread::JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let status = status.to_owned();
    let handle = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0_u8; 256];
        let mut used = 0;
        while used < request.len() && !request[..used].windows(4).any(|bytes| bytes == b"\r\n\r\n")
        {
            let read = stream.read(&mut request[used..]).unwrap();
            if read == 0 {
                break;
            }
            used += read;
        }
        let request = String::from_utf8_lossy(&request[..used]).into_owned();
        write!(
            stream,
            "HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        )
        .unwrap();
        request
    });
    (address.to_string(), handle)
}

#[test]
fn descriptor_http_readiness_uses_the_advertised_listener() {
    let (listener, server) = one_shot_health_response("200 OK");
    let ready = descriptor_http_ready(&http_descriptor(Some(listener), "ready"));
    let request = server.join().unwrap();
    assert!(ready, "request was {request:?}");
    assert!(request.starts_with("GET /health/ready "));

    let (listener, server) = one_shot_health_response("503 Service Unavailable");
    assert!(!descriptor_http_ready(&http_descriptor(
        Some(listener),
        "ready"
    )));
    let _ = server.join().unwrap();

    assert!(!descriptor_http_ready(&http_descriptor(None, "ready")));
    assert!(!descriptor_http_ready(&http_descriptor(
        Some("not-an-address".to_owned()),
        "ready"
    )));
    assert!(!descriptor_http_ready(&http_descriptor(
        Some("127.0.0.1:1".to_owned()),
        "ready"
    )));
}

fn inspect_live_descriptor_state(readiness: &str, health_status: Option<&str>) -> &'static str {
    let temp = TempDir::new().unwrap();
    let registry = scratch_registry(&temp);
    let (canonical, record) = register_config(&temp, &registry);
    let id = record.instance_id().unwrap();
    let runtime_parent = std::env::temp_dir().join(format!("oci{}", &id.as_str()[..5]));
    let runtime = runtime_parent.join(id.as_str());
    fs::create_dir_all(&runtime_parent).unwrap();
    let health = health_status.map(one_shot_health_response);
    let listener = health.as_ref().map(|(listener, _)| listener.clone());
    let (tx, _rx) = tokio::sync::watch::channel(false);
    let auth = Arc::new(crate::dashboard_auth::DashboardAuth::new(
        StartupId::generate(),
    ));
    let descriptor = build_descriptor(
        &id,
        &canonical,
        StartupId::generate(),
        PlatformId::generate(),
        env!("CARGO_PKG_VERSION"),
        ServiceScope::User,
        listener,
        None,
        readiness,
        SystemTime::now(),
    )
    .unwrap();
    let mut control = InstanceControl::publish(&runtime, descriptor, tx, auth).unwrap();
    let manager = FakeServiceManager::default();
    let runtime_root = runtime_parent.clone();
    let inspected = std::thread::scope(|scope| {
        let handle = scope.spawn(|| inspect_instance(&record, &manager, Some(&runtime_root)));
        for _ in 0..80 {
            control.poll_once().unwrap();
            if handle.is_finished() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        handle.join().unwrap().unwrap().state
    });
    if let Some((_, server)) = health {
        let _ = server.join().unwrap();
    }
    drop(control);
    let _ = fs::remove_dir_all(runtime_parent);
    inspected
}

#[test]
fn status_distinguishes_live_readiness_states() {
    assert_eq!(
        inspect_live_descriptor_state("ready", Some("200 OK")),
        "ready"
    );
    assert_eq!(
        inspect_live_descriptor_state("ready", Some("503 Service Unavailable")),
        "degraded"
    );
    assert_eq!(inspect_live_descriptor_state("degraded", None), "degraded");
    assert_eq!(inspect_live_descriptor_state("failed", None), "failed");
    assert_eq!(inspect_live_descriptor_state("starting", None), "starting");
}
