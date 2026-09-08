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

mod resolve_rejects_config_and_instance_together;

mod resolve_by_selector_and_config_path;

mod start_stop_restart_status_logs_and_remove_via_fake_manager;

mod start_by_selector_preserves_registered_system_identity;

mod start_rejects_mutual_exclusive_flags;

mod stop_without_active_service_best_effort_shutdown;

mod open_dashboard_no_open_with_control_socket;

mod open_dashboard_fails_when_not_ready;

mod open_dashboard_human_output_without_json;

mod dashboard_base_url_prefers_admin_and_normalizes_host;

mod status_reports_stopped_when_inactive;

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

mod select_running_returns_single_and_rejects_ambiguous;

mod select_running_none_reports_unregistered_discovery;

mod open_dashboard_uses_http_listener_prefix;

mod start_rejects_corrupt_registry_digest;

mod remove_rejects_live_control_socket;

mod wait_until_ready_times_out_without_descriptor;

mod wait_until_ready_accepts_fake_start_stub;

mod discover_unregistered_config_maps_not_found;

mod remove_rejects_when_manager_reports_active;

mod start_reports_already_running_when_active;

mod helper_io_and_open_url_paths;

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

mod descriptor_http_readiness_uses_the_advertised_listener;

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

mod status_distinguishes_live_readiness_states;
