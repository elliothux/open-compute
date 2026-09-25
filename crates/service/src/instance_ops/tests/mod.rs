use super::*;
use crate::instance_control::{CONTROL_SCHEMA_VERSION, GenerationDescriptor};
use crate::instance_control::{InstanceControl, build_descriptor};
use crate::instance_registry::{InstanceRegistry, ServiceScope};
use open_compute_core::clock::SystemClock;
use open_compute_core::{ErrorCode, InstanceId, StartupId};
use open_compute_storage::PlatformStorage;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};
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
[auth]
deployer_auth = {{ file = "{deployer}" }}
read_only_auth = {{ file = "{read_only}" }}

[data]
path = "{data}"
master_key_file = "{master}"

[storage]
backend = "local"
prefix = "system/"

[cache]
max_bytes = 1048576
high_watermark_ratio = 0.9
low_watermark_ratio = 0.8
max_artifact_bytes = 65536
"#,
        deployer = deployer.display(),
        read_only = read_only.display(),
        data = data.display(),
        master = master.display(),
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
    initialize_config(&canonical);
    let record = registry
        .register(&canonical, ServiceScope::User, SystemTime::now())
        .unwrap();
    (canonical, record)
}

fn initialize_config(config: &Path) {
    let loaded = load_platform_config_from(config, Path::new("/")).unwrap();
    drop(
        PlatformStorage::bootstrap_with_hardening(
            &loaded.config.data,
            &loaded.config.hardening,
            &SystemClock,
        )
        .unwrap(),
    );
}

mod resolve_rejects_config_and_instance_together;

mod resolve_by_selector_and_config_path;

mod open_dashboard_no_open_with_control_socket;

mod open_dashboard_fails_when_not_ready;

mod open_dashboard_human_output_without_json;

mod dashboard_base_url_prefers_admin_and_normalizes_host;

fn publish_ready_control(runtime: &Path, id: &InstanceId, canonical: &Path) -> InstanceControl {
    let (tx, _rx) = tokio::sync::watch::channel(false);
    let auth = Arc::new(crate::dashboard_auth::DashboardAuth::new(
        StartupId::generate(),
    ));
    let descriptor = build_descriptor(
        id,
        canonical,
        StartupId::generate(),
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

mod discover_unregistered_config_maps_not_found;

mod helper_io_and_open_url_paths;
