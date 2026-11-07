use super::*;
use crate::dashboard_auth::DashboardAuth;
use crate::instance_control::{InstanceControl, build_descriptor, runtime_dir_for};
use crate::instance_registry::ServiceScope;
use crate::target_http::TargetHttp;
use crate::target_registry::TargetRegistry;
use open_compute_core::StartupId;
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

const TEST_INSTANCE_ID: &str = "01890f3c8b407cc0a000000000000001";

#[derive(Default)]
struct FixtureHttp(Mutex<HashMap<String, Vec<u8>>>);

impl FixtureHttp {
    fn capabilities(&self, base: &str, version: &str) {
        self.0.lock().unwrap().insert(
            format!("{base}/open-compute/capabilities"),
            serde_json::to_vec(&serde_json::json!({
                "success": true,
                "result": {"cf_version": version}
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

/// Install a fake `cf` package whose `.bin/cf` is a POSIX shell shim, as some package managers write.
fn write_fake_cf(node_modules: &Path, version: &str) {
    let package = node_modules.join("cf");
    fs::create_dir_all(package.join("bin")).unwrap();
    fs::write(
        package.join("package.json"),
        r#"{"name":"cf","bin":{"cf":"./bin/cf","cloudflare":"./bin/cf"}}"#,
    )
    .unwrap();
    fs::write(
        package.join("bin/cf"),
        format!("if (process.argv[2] === '--version') {{ console.log('{version}'); console.error('runtime diagnostic'); }}\n"),
    )
    .unwrap();
    let shim = node_modules.join(".bin/cf");
    fs::create_dir_all(shim.parent().unwrap()).unwrap();
    fs::write(
        &shim,
        "#!/bin/sh\nexec node \"$(dirname \"$0\")/../cf/bin/cf\" \"$@\"\n",
    )
    .unwrap();
    fs::set_permissions(&shim, fs::Permissions::from_mode(0o755)).unwrap();
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
            TEST_INSTANCE_ID.parse().unwrap(),
            token,
            SystemTime::now(),
        )
        .unwrap();
    let workspace = temp.path().join("workspace");
    let project = workspace.join("packages/worker");
    fs::create_dir_all(&project).unwrap();
    fs::write(project.join("cloudflare.config.ts"), "export default {};\n").unwrap();
    write_fake_cf(&workspace.join("node_modules"), version);
    let http = FixtureHttp::default();
    http.capabilities("https://compute.example/client/v4", "1.0.0-beta.12");
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
[auth]
deployer_auth = {{ file = "{}" }}
read_only_auth = {{ file = "{}" }}

[data]
path = "{}"
master_key_file = "{}"

[storage]
backend = "local"
prefix = "system/"
"#,
            root.join("deployer.token").display(),
            root.join("read-only.token").display(),
            data.display(),
            root.join("master.key").display(),
        ),
    )
    .unwrap();
    let loaded = load_platform_config_from(&config, root).unwrap();
    drop(
        open_compute_storage::PlatformStorage::bootstrap(
            &loaded.config.data,
            &open_compute_core::clock::SystemClock,
        )
        .unwrap(),
    );
    config
}

#[tokio::test]
async fn remote_launch_preserves_opaque_args_and_uses_nearest_hoisted_binary() {
    let (temp, registry, http, project, name) = remote_fixture("1.0.0-beta.12");
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
    let launch = prepare_cf_launch(
        Some(&name),
        None,
        None,
        Some(&project),
        &arguments,
        temp.path(),
        &instances,
        ServiceScope::User,
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
            .join("node_modules/cf/bin/cf")
    );
    assert_eq!(launch.target_kind, "target");
    assert_eq!(launch.cf_version, "1.0.0-beta.12");
    assert_eq!(launch.certified_cf_version, "1.0.0-beta.12");
    assert!(diagnostic.is_empty());

    let mut failed_diagnostic = std::io::Cursor::new(&mut [][..]);
    let error = launch.clone().exec(&mut failed_diagnostic).unwrap_err();
    assert_eq!(error.code(), ErrorCode::CfInvalid);

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
        Some(&Some(OsString::from(TEST_INSTANCE_ID)))
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
        environment.get(OsStr::new("CF_SEND_TELEMETRY")),
        Some(&Some(OsString::from("false")))
    );
    assert_eq!(
        environment.get(OsStr::new("DO_NOT_TRACK")),
        Some(&Some(OsString::from("1")))
    );
}

#[tokio::test]
async fn same_release_family_drift_launches_without_a_warning() {
    let (temp, registry, http, project, name) = remote_fixture("1.0.0-beta.10");
    let instances = InstanceRegistry::with_roots(
        temp.path().join("instances/system"),
        temp.path().join("instances/user"),
    );
    let mut diagnostic = Vec::new();
    let launch = prepare_cf_launch(
        Some(&name),
        None,
        None,
        Some(&project),
        &[OsString::from("deploy")],
        temp.path(),
        &instances,
        ServiceScope::User,
        &registry,
        &http,
        None,
        &mut diagnostic,
    )
    .await
    .unwrap();
    assert_eq!(launch.cf_version, "1.0.0-beta.10");
    assert_eq!(launch.certified_cf_version, "1.0.0-beta.12");
    assert!(diagnostic.is_empty());
}

#[tokio::test]
async fn other_release_families_warn_but_still_launch() {
    for version in ["0.9.9", "1.1.0-beta.1", "2.0.0-beta.1"] {
        let (temp, registry, http, project, name) = remote_fixture(version);
        let instances = InstanceRegistry::with_roots(
            temp.path().join("instances/system"),
            temp.path().join("instances/user"),
        );
        let mut diagnostic = Vec::new();
        let launch = prepare_cf_launch(
            Some(&name),
            None,
            None,
            Some(&project),
            &[OsString::from("deploy")],
            temp.path(),
            &instances,
            ServiceScope::User,
            &registry,
            &http,
            None,
            &mut diagnostic,
        )
        .await
        .unwrap();
        assert_eq!(launch.cf_version, version);
        assert_eq!(launch.certified_cf_version, "1.0.0-beta.12");
        let diagnostic = String::from_utf8(diagnostic).unwrap();
        assert!(diagnostic.contains("CF_VERSION_WARNING"));
        assert!(diagnostic.contains(&format!("path={}", launch.executable.display())));
        assert!(diagnostic.contains(&format!("detected={version} certified=1.0.0-beta.12")));
        assert!(!diagnostic.contains("test-deployer-token"));
    }
}

#[tokio::test]
async fn failed_version_process_remains_a_hard_failure() {
    let (temp, registry, http, project, name) = remote_fixture("1.0.0-beta.12");
    let executable = temp.path().join("workspace/node_modules/cf/bin/cf");
    fs::write(&executable, "process.exit(23);\n").unwrap();
    let instances = InstanceRegistry::with_roots(
        temp.path().join("instances/system"),
        temp.path().join("instances/user"),
    );
    let mut diagnostic = Vec::new();
    let error = prepare_cf_launch(
        Some(&name),
        None,
        None,
        Some(&project),
        &[OsString::from("deploy")],
        temp.path(),
        &instances,
        ServiceScope::User,
        &registry,
        &http,
        None,
        &mut diagnostic,
    )
    .await
    .unwrap_err();
    assert_eq!(error.code(), ErrorCode::CfInvalid);
    assert!(diagnostic.is_empty());
}

#[tokio::test]
async fn unusable_binary_empty_arguments_and_selector_conflicts_fail_closed() {
    let (temp, registry, http, project, name) = remote_fixture("1.0.0-beta.12");
    let package = temp.path().join("workspace/node_modules/cf");
    fs::remove_file(package.join("bin/cf")).unwrap();
    let instances = InstanceRegistry::with_roots(
        temp.path().join("instances/system"),
        temp.path().join("instances/user"),
    );
    let mut diagnostic = Vec::new();
    let missing = prepare_cf_launch(
        Some(&name),
        None,
        None,
        Some(&project),
        &[OsString::from("deploy")],
        temp.path(),
        &instances,
        ServiceScope::User,
        &registry,
        &http,
        None,
        &mut diagnostic,
    )
    .await;
    assert_eq!(missing.unwrap_err().code(), ErrorCode::CfInvalid);
    let empty = prepare_cf_launch(
        Some(&name),
        None,
        None,
        Some(&project),
        &[],
        temp.path(),
        &instances,
        ServiceScope::User,
        &registry,
        &http,
        None,
        &mut diagnostic,
    )
    .await;
    assert_eq!(empty.unwrap_err().code(), ErrorCode::CfInvalid);
    let selector: InstanceSelector = TEST_INSTANCE_ID.parse().unwrap();
    let conflict = prepare_cf_launch(
        Some(&name),
        None,
        Some(&selector),
        Some(&project),
        &[OsString::from("deploy")],
        temp.path(),
        &instances,
        ServiceScope::User,
        &registry,
        &http,
        None,
        &mut diagnostic,
    )
    .await;
    assert_eq!(conflict.unwrap_err().code(), ErrorCode::CfInvalid);
    let config_conflict = prepare_cf_launch(
        Some(&name),
        Some(Path::new("/tmp/compute.toml")),
        None,
        Some(&project),
        &[OsString::from("deploy")],
        temp.path(),
        &instances,
        ServiceScope::User,
        &registry,
        &http,
        None,
        &mut diagnostic,
    )
    .await;
    assert_eq!(config_conflict.unwrap_err().code(), ErrorCode::CfInvalid);
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
    let id = record.instance_id().unwrap();
    // Keep the override root on `/tmp`: macOS sockaddr_un caps paths at 103 bytes, and
    // tempfile directories under TMPDIR routinely exceed that once instance_id and
    // control.sock are appended (same constraint as fallback_user_runtime_root).
    let runtime_root = PathBuf::from("/tmp").join(format!("oc-wrl-{}", std::process::id()));
    let _ = fs::remove_dir_all(&runtime_root);
    let runtime = runtime_dir_for(ServiceScope::User, &id, Some(&runtime_root)).unwrap();
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
    let mut control = InstanceControl::publish(&runtime, descriptor, shutdown, auth, true).unwrap();
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
    fs::write(project.join("cloudflare.config.ts"), "export default {};\n").unwrap();
    write_fake_cf(&project.join("node_modules"), "1.0.0-beta.12");
    let targets = TargetRegistry::at(temp.path().join("targets/targets.toml"));
    let http = FixtureHttp::default();
    http.capabilities("http://127.0.0.1:8787/client/v4", "1.0.0-beta.12");
    let mut diagnostic = Vec::new();
    let launch = prepare_cf_launch(
        None,
        None,
        None,
        Some(&project),
        &[OsString::from("deploy")],
        temp.path(),
        &instances,
        ServiceScope::User,
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
    assert_eq!(launch.instance_id.as_str(), record.instance_id);
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
        instance_id: TEST_INSTANCE_ID.to_owned(),
        canonical_config_path: "/tmp/compute.toml".to_owned(),
        startup_id: StartupId::generate().to_string(),
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

#[test]
fn project_cf_resolves_the_package_entry_through_symlinks_and_rejects_escapes() {
    let temp = TempDir::new().unwrap();
    let project = temp.path().join("project");
    let store = project.join("node_modules/.store/cf@1/node_modules");
    write_fake_cf(&store, "1.0.0-beta.12");
    fs::create_dir_all(project.join("node_modules/.bin")).unwrap();
    std::os::unix::fs::symlink(store.join("cf"), project.join("node_modules/cf")).unwrap();
    std::os::unix::fs::symlink("../cf/bin/cf", project.join("node_modules/.bin/cf")).unwrap();
    let nested = project.join("packages/worker");
    fs::create_dir_all(&nested).unwrap();
    let (entry, tools) = resolve_project_cf(&nested).unwrap();
    assert_eq!(entry, store.join("cf/bin/cf").canonicalize().unwrap());
    assert_eq!(tools, project.join("node_modules/.bin"));

    let package = store.join("cf");
    for (manifest, expected) in [
        (r#"{"name":"cf","bin":"./bin/cf"}"#, true),
        (r#"{"name":"cf","bin":{"cloudflare":"./bin/cf"}}"#, false),
        (r#"{"name":"cf","bin":{"cf":"../../../outside.js"}}"#, false),
        (r#"{"name":"cf","bin":{"cf":"/etc/hosts"}}"#, false),
        (r#"{"name":"other","bin":"./bin/cf"}"#, false),
        (r#"{"name":"cf"}"#, false),
    ] {
        fs::write(project.join("node_modules/.store/cf@1/outside.js"), "").unwrap();
        fs::write(package.join("package.json"), manifest).unwrap();
        let resolved = resolve_project_cf(&nested);
        assert_eq!(resolved.is_ok(), expected, "{manifest}");
        if let Err(error) = resolved {
            assert_eq!(error.code(), ErrorCode::CfInvalid);
        }
    }

    fs::write(
        package.join("package.json"),
        r#"{"name":"cf","bin":"./bin/link"}"#,
    )
    .unwrap();
    std::os::unix::fs::symlink(temp.path().join("escape.js"), package.join("bin/link")).unwrap();
    fs::write(temp.path().join("escape.js"), "").unwrap();
    assert_eq!(
        resolve_project_cf(&nested).unwrap_err().code(),
        ErrorCode::CfInvalid
    );
}
