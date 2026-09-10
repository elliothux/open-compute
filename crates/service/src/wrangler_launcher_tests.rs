use super::*;
use crate::dashboard_auth::DashboardAuth;
use crate::instance_control::{InstanceControl, build_descriptor, runtime_dir_for};
use crate::instance_registry::ServiceScope;
use crate::target_http::TargetHttp;
use crate::target_registry::TargetRegistry;
use open_compute_core::{InstanceId, PlatformId, StartupId};
use std::collections::HashMap;
use std::ffi::OsStr;
use std::future::Future;
use std::os::unix::fs::PermissionsExt;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::SystemTime;
use tempfile::TempDir;

#[derive(Default)]
struct FixtureHttp(Mutex<HashMap<String, Vec<u8>>>);

impl FixtureHttp {
    fn capabilities(&self, base: &str, version: &str) {
        self.0.lock().unwrap().insert(
            format!("{base}/open-compute/capabilities"),
            serde_json::to_vec(&serde_json::json!({
                "success": true,
                "result": {"wrangler_version": version}
            }))
            .unwrap(),
        );
    }
}

impl TargetHttp for FixtureHttp {
    fn get<'a>(
        &'a self,
        url: &'a str,
        _token: &'a SecretString,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<u8>, PlatformError>> + Send + 'a>> {
        Box::pin(async move {
            self.0.lock().unwrap().get(url).cloned().ok_or_else(|| {
                PlatformError::new(ErrorCode::PlatformUnavailable, "fixture missing")
            })
        })
    }
}

fn write_fake_wrangler(path: &Path, version: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(
        path,
        format!(
            "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then printf '%s\\n' '{version}'; printf '%s\\n' 'runtime diagnostic' >&2; exit 0; fi\nexit 0\n"
        ),
    )
    .unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

fn remote_fixture(version: &str) -> (TempDir, TargetRegistry, FixtureHttp, PathBuf, TargetName) {
    let temp = TempDir::new().unwrap();
    let target_root = temp.path().join("target-registry");
    fs::create_dir(&target_root).unwrap();
    fs::set_permissions(&target_root, fs::Permissions::from_mode(0o700)).unwrap();
    let token = temp.path().join("deployer.token");
    fs::write(&token, "test-deployer-token\n").unwrap();
    fs::set_permissions(&token, fs::Permissions::from_mode(0o600)).unwrap();
    let registry = TargetRegistry::at(target_root.join("targets.toml"));
    let name: TargetName = "remote".parse().unwrap();
    registry
        .add(
            name.clone(),
            "https://compute.example/client/v4".parse().unwrap(),
            "0123456789abcdef0123456789abcdef".parse().unwrap(),
            token,
            SystemTime::now(),
        )
        .unwrap();
    let workspace = temp.path().join("workspace");
    let project = workspace.join("packages/worker");
    fs::create_dir_all(&project).unwrap();
    write_fake_wrangler(&workspace.join("node_modules/.bin/wrangler"), version);
    let http = FixtureHttp::default();
    http.capabilities("https://compute.example/client/v4", "4.127.1");
    (temp, registry, http, project, name)
}

fn write_local_config(root: &Path) -> PathBuf {
    let data = root.join("data");
    let objects = root.join("objects");
    fs::create_dir_all(&data).unwrap();
    fs::create_dir_all(&objects).unwrap();
    for (name, value) in [
        ("admin.token", "admin-token"),
        ("deployer.token", "local-deployer-token"),
        ("read-only.token", "read-only-token"),
    ] {
        let path = root.join(name);
        fs::write(&path, format!("{value}\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    }
    let config = root.join("compute.toml");
    fs::write(
        &config,
        format!(
            r#"
[server]
public_bind = "127.0.0.1:8787"
admin_auth = {{ file = "{}" }}
deployer_auth = {{ file = "{}" }}
read_only_auth = {{ file = "{}" }}

[data]
path = "{}"
master_key_file = "{}"

[storage]
backend = "local"
path = "{}"
prefix = "system/"
"#,
            root.join("admin.token").display(),
            root.join("deployer.token").display(),
            root.join("read-only.token").display(),
            data.display(),
            root.join("master.key").display(),
            objects.display(),
        ),
    )
    .unwrap();
    config
}

#[tokio::test]
async fn remote_launch_preserves_opaque_args_and_uses_nearest_hoisted_binary() {
    let (temp, registry, http, project, name) = remote_fixture("4.127.1");
    let instances = InstanceRegistry::with_roots(
        temp.path().join("instances/system"),
        temp.path().join("instances/user"),
    );
    let arguments = vec![
        OsString::from("deploy"),
        OsString::from("--config"),
        OsString::from("配置.jsonc"),
        OsString::new(),
        OsString::from("--cwd"),
        OsString::from("nested"),
    ];
    let mut diagnostic = Vec::new();
    let launch = prepare_wrangler_launch(
        Some(&name),
        None,
        None,
        Some(&project),
        &arguments,
        temp.path(),
        &instances,
        &registry,
        &http,
        None,
        &mut diagnostic,
    )
    .await
    .unwrap();
    assert_eq!(launch.arguments, arguments);
    assert_eq!(launch.cwd, project.canonicalize().unwrap());
    assert_eq!(
        launch.executable,
        temp.path()
            .join("workspace")
            .canonicalize()
            .unwrap()
            .join("node_modules/.bin/wrangler")
    );
    assert_eq!(launch.target_kind, "target");
    assert_eq!(launch.wrangler_version, "4.127.1");
    assert_eq!(launch.certified_wrangler_version, "4.127.1");
    assert!(diagnostic.is_empty());

    let command = launch.child_command();
    let environment = command
        .get_envs()
        .map(|(name, value)| (name.to_owned(), value.map(OsStr::to_owned)))
        .collect::<HashMap<_, _>>();
    assert_eq!(
        environment.get(OsStr::new("CLOUDFLARE_API_BASE_URL")),
        Some(&Some(OsString::from("https://compute.example/client/v4")))
    );
    assert_eq!(
        environment.get(OsStr::new("CLOUDFLARE_ACCOUNT_ID")),
        Some(&Some(OsString::from("0123456789abcdef0123456789abcdef")))
    );
    assert!(
        environment
            .get(OsStr::new("CLOUDFLARE_API_TOKEN"))
            .and_then(Option::as_ref)
            .is_some_and(|value| value == "test-deployer-token")
    );
    for name in REMOVED_ENVIRONMENT {
        assert_eq!(environment.get(OsStr::new(name)), Some(&None));
    }
    assert_eq!(
        environment.get(OsStr::new("WRANGLER_LOG_SANITIZE")),
        Some(&Some(OsString::from("true")))
    );
    assert_eq!(
        environment.get(OsStr::new("WRANGLER_SEND_METRICS")),
        Some(&Some(OsString::from("false")))
    );
    assert_eq!(
        environment.get(OsStr::new("WRANGLER_SEND_ERROR_REPORTS")),
        Some(&Some(OsString::from("false")))
    );
}

#[tokio::test]
async fn same_major_version_drift_launches_without_a_warning() {
    let (temp, registry, http, project, name) = remote_fixture("4.130.0");
    let instances = InstanceRegistry::with_roots(
        temp.path().join("instances/system"),
        temp.path().join("instances/user"),
    );
    let mut diagnostic = Vec::new();
    let launch = prepare_wrangler_launch(
        Some(&name),
        None,
        None,
        Some(&project),
        &[OsString::from("deploy")],
        temp.path(),
        &instances,
        &registry,
        &http,
        None,
        &mut diagnostic,
    )
    .await
    .unwrap();
    assert_eq!(launch.wrangler_version, "4.130.0");
    assert_eq!(launch.certified_wrangler_version, "4.127.1");
    assert!(diagnostic.is_empty());
}

#[tokio::test]
async fn cross_major_version_drift_warns_but_still_launches() {
    let (temp, registry, http, project, name) = remote_fixture("5.0.0");
    let instances = InstanceRegistry::with_roots(
        temp.path().join("instances/system"),
        temp.path().join("instances/user"),
    );
    let mut diagnostic = Vec::new();
    let launch = prepare_wrangler_launch(
        Some(&name),
        None,
        None,
        Some(&project),
        &[OsString::from("deploy")],
        temp.path(),
        &instances,
        &registry,
        &http,
        None,
        &mut diagnostic,
    )
    .await
    .unwrap();
    assert_eq!(launch.wrangler_version, "5.0.0");
    assert_eq!(launch.certified_wrangler_version, "4.127.1");
    let diagnostic = String::from_utf8(diagnostic).unwrap();
    assert!(diagnostic.contains("WRANGLER_MAJOR_VERSION_MISMATCH"));
    assert!(diagnostic.contains(&format!("path={}", launch.executable.display())));
    assert!(diagnostic.contains("detected=5.0.0 certified=4.127.1"));
    assert!(!diagnostic.contains("test-deployer-token"));
}

#[tokio::test]
async fn failed_version_process_remains_a_hard_failure() {
    let (temp, registry, http, project, name) = remote_fixture("4.127.1");
    let executable = temp.path().join("workspace/node_modules/.bin/wrangler");
    fs::write(&executable, "#!/bin/sh\nexit 23\n").unwrap();
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
    let instances = InstanceRegistry::with_roots(
        temp.path().join("instances/system"),
        temp.path().join("instances/user"),
    );
    let mut diagnostic = Vec::new();
    let error = prepare_wrangler_launch(
        Some(&name),
        None,
        None,
        Some(&project),
        &[OsString::from("deploy")],
        temp.path(),
        &instances,
        &registry,
        &http,
        None,
        &mut diagnostic,
    )
    .await
    .unwrap_err();
    assert_eq!(error.code(), ErrorCode::WranglerInvalid);
    assert!(diagnostic.is_empty());
}

#[tokio::test]
async fn unusable_binary_empty_arguments_and_selector_conflicts_fail_closed() {
    let (temp, registry, http, project, name) = remote_fixture("4.127.1");
    let executable = temp.path().join("workspace/node_modules/.bin/wrangler");
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o600)).unwrap();
    let instances = InstanceRegistry::with_roots(
        temp.path().join("instances/system"),
        temp.path().join("instances/user"),
    );
    let mut diagnostic = Vec::new();
    let missing = prepare_wrangler_launch(
        Some(&name),
        None,
        None,
        Some(&project),
        &[OsString::from("deploy")],
        temp.path(),
        &instances,
        &registry,
        &http,
        None,
        &mut diagnostic,
    )
    .await;
    assert_eq!(missing.unwrap_err().code(), ErrorCode::WranglerInvalid);
    let empty = prepare_wrangler_launch(
        Some(&name),
        None,
        None,
        Some(&project),
        &[],
        temp.path(),
        &instances,
        &registry,
        &http,
        None,
        &mut diagnostic,
    )
    .await;
    assert_eq!(empty.unwrap_err().code(), ErrorCode::WranglerInvalid);
    let selector: InstanceSelector = "abcde".parse().unwrap();
    let conflict = prepare_wrangler_launch(
        Some(&name),
        None,
        Some(&selector),
        Some(&project),
        &[OsString::from("deploy")],
        temp.path(),
        &instances,
        &registry,
        &http,
        None,
        &mut diagnostic,
    )
    .await;
    assert_eq!(conflict.unwrap_err().code(), ErrorCode::WranglerInvalid);
    let config_conflict = prepare_wrangler_launch(
        Some(&name),
        Some(Path::new("/tmp/compute.toml")),
        None,
        Some(&project),
        &[OsString::from("deploy")],
        temp.path(),
        &instances,
        &registry,
        &http,
        None,
        &mut diagnostic,
    )
    .await;
    assert_eq!(
        config_conflict.unwrap_err().code(),
        ErrorCode::WranglerInvalid
    );
}

#[tokio::test]
async fn unique_running_local_instance_supplies_config_token_listener_and_account() {
    let temp = TempDir::new().unwrap();
    let config = write_local_config(temp.path());
    let canonical = config.canonicalize().unwrap();
    let instances = InstanceRegistry::with_roots(
        temp.path().join("instances/system"),
        temp.path().join("instances/user"),
    );
    let record = instances
        .register(&canonical, ServiceScope::User, SystemTime::now())
        .unwrap();
    let id = InstanceId::from_canonical_config_path(&canonical).unwrap();
    // Keep the override root on `/tmp`: macOS sockaddr_un caps paths at 103 bytes, and
    // tempfile directories under TMPDIR routinely exceed that once instance_id and
    // control.sock are appended (same constraint as fallback_user_runtime_root).
    let runtime_root = PathBuf::from("/tmp").join(format!("oc-wrl-{}", std::process::id()));
    let _ = fs::remove_dir_all(&runtime_root);
    let runtime = runtime_dir_for(ServiceScope::User, &id, Some(&runtime_root));
    assert!(
        runtime
            .join("control.sock")
            .as_os_str()
            .as_encoded_bytes()
            .len()
            <= 103,
        "control socket path must fit macOS sockaddr_un"
    );
    let startup_id = StartupId::generate();
    let descriptor = build_descriptor(
        &id,
        &canonical,
        startup_id,
        PlatformId::generate(),
        "0123456789abcdef0123456789abcdef".to_owned(),
        env!("CARGO_PKG_VERSION"),
        ServiceScope::User,
        Some("127.0.0.1:8787".to_owned()),
        None,
        "ready",
        SystemTime::now(),
    )
    .unwrap();
    let (shutdown, _shutdown_rx) = tokio::sync::watch::channel(false);
    let auth = Arc::new(DashboardAuth::new(startup_id));
    let mut control = InstanceControl::publish(&runtime, descriptor, shutdown, auth).unwrap();
    let polling = Arc::new(AtomicBool::new(true));
    let poll_flag = polling.clone();
    let control_thread = std::thread::spawn(move || {
        while poll_flag.load(Ordering::Acquire) {
            control.poll_once().unwrap();
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    });

    let project = temp.path().join("project");
    fs::create_dir(&project).unwrap();
    write_fake_wrangler(&project.join("node_modules/.bin/wrangler"), "4.127.1");
    let targets = TargetRegistry::at(temp.path().join("targets/targets.toml"));
    let http = FixtureHttp::default();
    http.capabilities("http://127.0.0.1:8787/client/v4", "4.127.1");
    let mut diagnostic = Vec::new();
    let launch = prepare_wrangler_launch(
        None,
        None,
        None,
        Some(&project),
        &[OsString::from("deploy")],
        temp.path(),
        &instances,
        &targets,
        &http,
        Some(&runtime_root),
        &mut diagnostic,
    )
    .await
    .unwrap();
    assert_eq!(launch.target_kind, "instance");
    assert_eq!(launch.target_name, record.instance_id);
    assert_eq!(launch.api_base_url, "http://127.0.0.1:8787/client/v4");
    assert_eq!(
        launch.account_id.as_str(),
        "0123456789abcdef0123456789abcdef"
    );
    assert!(
        launch
            .child_command()
            .get_envs()
            .any(|(name, value)| name == "CLOUDFLARE_API_TOKEN"
                && value.is_some_and(|value| value == "local-deployer-token"))
    );

    polling.store(false, Ordering::Release);
    control_thread.join().unwrap();
    let _ = fs::remove_dir_all(&runtime_root);
}

#[test]
fn local_api_origin_uses_only_the_configured_admin_surface() {
    let descriptor = GenerationDescriptor {
        schema_version: CONTROL_SCHEMA_VERSION,
        instance_id: "abcde".to_owned(),
        canonical_config_path: "/tmp/compute.toml".to_owned(),
        startup_id: StartupId::generate().to_string(),
        platform_id: PlatformId::generate().to_string(),
        account_id: "0123456789abcdef0123456789abcdef".to_owned(),
        release_version: env!("CARGO_PKG_VERSION").to_owned(),
        service_scope: ServiceScope::User,
        public_listener: Some("0.0.0.0:8787".to_owned()),
        admin_listener: None,
        readiness: "ready".to_owned(),
        published_at: 1,
    };
    assert_eq!(
        instance_api_base_url(&descriptor, false).unwrap(),
        "http://127.0.0.1:8787/client/v4"
    );
    assert_eq!(
        instance_api_base_url(&descriptor, true).unwrap_err().code(),
        ErrorCode::PlatformUnavailable
    );
    let descriptor = GenerationDescriptor {
        admin_listener: Some("[::]:8788".to_owned()),
        ..descriptor
    };
    assert_eq!(
        instance_api_base_url(&descriptor, true).unwrap(),
        "http://[::1]:8788/client/v4"
    );
}
