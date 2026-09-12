use super::*;
use crate::service_manager::FakeServiceManager;
use std::io::Cursor;
use std::os::unix::fs::PermissionsExt;
use tempfile::TempDir;

fn test_options(temp: &TempDir) -> (SetupOptions, PathBuf) {
    let root = temp.path().to_path_buf();
    let config_parent = root.join("etc/open-compute");
    let data_dir = root.join("var/lib/open-compute");
    let secrets_dir = data_dir.join("secrets");
    let system_registry = root.join("registry/system");
    let user_registry = root.join("registry/user");
    let config_path = config_parent.join("config.toml");
    (
        SetupOptions {
            config: Some(config_path.clone()),
            yes: true,
            roots: SetupRoots {
                config_parent,
                data_dir,
                secrets_dir,
                system_registry_root: system_registry,
                user_registry_root: user_registry,
            },
            config_path: config_path.clone(),
            scope: ServiceScope::User,
        },
        config_path,
    )
}

#[test]
fn setup_yes_refuses_existing_config() {
    let temp = TempDir::new().unwrap();
    let (options, config_path) = test_options(&temp);
    fs::create_dir_all(config_path.parent().unwrap()).unwrap();
    fs::write(&config_path, b"already\n").unwrap();
    let fake = FakeServiceManager::default();
    let err = run_setup(&options, temp.path(), &fake, &mut Vec::new()).unwrap_err();
    assert_eq!(err.code(), ErrorCode::PathInvalid);
    assert!(err.message().contains("refusing to overwrite"));
}

#[test]
fn setup_yes_refuses_existing_secret() {
    let temp = TempDir::new().unwrap();
    let (options, _) = test_options(&temp);
    fs::create_dir_all(&options.roots.secrets_dir).unwrap();
    fs::write(options.roots.secrets_dir.join("admin.token"), b"x\n").unwrap();
    let err = run_setup(
        &options,
        temp.path(),
        &FakeServiceManager::default(),
        &mut Vec::new(),
    )
    .unwrap_err();
    assert_eq!(err.code(), ErrorCode::PathInvalid);
}

#[test]
fn setup_yes_writes_secrets_and_loadable_config() {
    let temp = TempDir::new().unwrap();
    let (options, config_path) = test_options(&temp);
    let fake = FakeServiceManager::default();
    let mut out = Vec::new();
    run_setup(&options, temp.path(), &fake, &mut out).unwrap();

    let admin = options.roots.secrets_dir.join("admin.token");
    let deployer = options.roots.secrets_dir.join("deployer.token");
    let read_only = options.roots.secrets_dir.join("read-only.token");
    for path in [&admin, &deployer, &read_only] {
        let meta = fs::symlink_metadata(path).unwrap();
        assert_eq!(meta.permissions().mode() & 0o777, 0o600);
        let body = fs::read_to_string(path).unwrap();
        assert!(!body.trim().is_empty());
    }
    let admin_body = fs::read_to_string(&admin).unwrap();
    let deployer_body = fs::read_to_string(&deployer).unwrap();
    let read_only_body = fs::read_to_string(&read_only).unwrap();
    assert_ne!(admin_body, deployer_body);
    assert_ne!(admin_body, read_only_body);
    assert_ne!(deployer_body, read_only_body);

    assert!(!options.roots.data_dir.join("keys/master.key").exists());
    let loaded = load_platform_config_from(&config_path, temp.path()).unwrap();
    assert_eq!(
        loaded.config.server.admin_auth.file.as_deref(),
        Some(admin.as_path())
    );
    assert!(loaded.config.server.admin_auth.env.is_none());
    assert!(loaded.config.dashboard.enabled);

    let text = String::from_utf8(out).unwrap();
    assert!(text.contains("SETUP_OK"));
    assert!(text.contains("ocd dashboard"));
    assert!(!fake.installed().is_empty());
    assert!(!fake.started().is_empty());
}

#[test]
fn setup_yes_registers_and_starts_via_fake_manager() {
    let temp = TempDir::new().unwrap();
    let (options, _) = test_options(&temp);
    let fake = FakeServiceManager::default();
    run_setup(&options, temp.path(), &fake, &mut Vec::new()).unwrap();
    assert_eq!(fake.installed().len(), 1);
    assert_eq!(fake.started().len(), 1);
    assert_eq!(fake.installed(), fake.started());

    let registry = InstanceRegistry::with_roots(
        options.roots.system_registry_root.clone(),
        options.roots.user_registry_root.clone(),
    );
    let listed = registry.list().unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].service_scope, ServiceScope::User);
}

#[test]
fn setup_install_failure_rolls_back_published_files_and_registry() {
    let temp = TempDir::new().unwrap();
    let (options, config_path) = test_options(&temp);
    let fake = FakeServiceManager::default();
    fake.set_fail_install(true);
    let err = run_setup(&options, temp.path(), &fake, &mut Vec::new()).unwrap_err();
    assert_eq!(err.code(), ErrorCode::PlatformUnavailable);
    assert!(!config_path.exists());
    assert!(!options.roots.secrets_dir.join("admin.token").exists());
    assert!(!options.roots.secrets_dir.join("deployer.token").exists());
    assert!(!options.roots.secrets_dir.join("read-only.token").exists());
    let registry = InstanceRegistry::with_roots(
        options.roots.system_registry_root.clone(),
        options.roots.user_registry_root.clone(),
    );
    assert!(registry.list().unwrap().is_empty());
    assert!(fake.installed().is_empty());
}

#[test]
fn setup_without_yes_requires_tty() {
    let temp = TempDir::new().unwrap();
    let (mut options, _) = test_options(&temp);
    options.yes = false;
    let err = run_setup(
        &options,
        temp.path(),
        &FakeServiceManager::default(),
        &mut Vec::new(),
    )
    .unwrap_err();
    assert_eq!(err.code(), ErrorCode::ConfigInvalid);
    assert!(err.message().contains("non-interactive"));
}

#[test]
fn setup_roots_production_rejects_system_with_config() {
    let err = SetupRoots::production(Path::new("/tmp"), Some(Path::new("compute.toml")), true)
        .unwrap_err();
    assert_eq!(err.code(), ErrorCode::ConfigPathInvalid);
}

#[test]
fn setup_roots_production_with_config_uses_user_scope() {
    let temp = TempDir::new().unwrap();
    let config = temp.path().join("project/compute.toml");
    fs::create_dir_all(config.parent().unwrap()).unwrap();
    let (roots, path, scope) =
        SetupRoots::production(temp.path(), Some(Path::new("project/compute.toml")), false)
            .unwrap();
    assert_eq!(scope, ServiceScope::User);
    assert_eq!(path, config);
    assert!(roots.data_dir.ends_with(".data/open-compute"));
    assert!(roots.secrets_dir.ends_with("secrets"));
}

#[test]
fn interactive_plan_accepts_defaults_without_start() {
    let temp = TempDir::new().unwrap();
    let (options, _) = test_options(&temp);
    let mut input = Cursor::new(b"\n\n\n\n\nn\n".to_vec());
    let mut prompts = Vec::new();
    let plan = plan_interactive(&options, temp.path(), &mut input, &mut prompts).unwrap();
    assert_eq!(plan.scope, ServiceScope::User);
    assert!(!plan.start_service);
    assert!(plan.dashboard_enabled);
    let fake = FakeServiceManager::default();
    let mut out = Vec::new();
    execute_plan(&plan, temp.path(), &fake, &mut out).unwrap();
    assert!(fake.installed().is_empty());
    let registry = InstanceRegistry::with_roots(
        options.roots.system_registry_root.clone(),
        options.roots.user_registry_root.clone(),
    );
    assert!(registry.list().unwrap().is_empty());
    let text = String::from_utf8(out).unwrap();
    assert!(text.contains("SETUP_OK"));
    assert!(text.contains("registered=false"));
    assert!(text.contains("ocd start --config"));
}

#[test]
fn interactive_plan_rejects_s3() {
    let temp = TempDir::new().unwrap();
    let (options, _) = test_options(&temp);
    let err = plan_interactive(
        &options,
        temp.path(),
        &mut Cursor::new(b"\n\n\ns3\n".to_vec()),
        &mut Vec::new(),
    )
    .unwrap_err();
    assert!(err.message().contains("local object backend"));
}

#[test]
fn interactive_plan_rejects_bad_bool() {
    let temp = TempDir::new().unwrap();
    let (options, _) = test_options(&temp);
    let err = plan_interactive(
        &options,
        temp.path(),
        &mut Cursor::new(b"\n\n\n\nmaybe\n".to_vec()),
        &mut Vec::new(),
    )
    .unwrap_err();
    assert!(err.message().contains("expected y/n"));
}

#[test]
fn map_privilege_system_permission_message() {
    let err = map_privilege(
        &std::io::Error::from(std::io::ErrorKind::PermissionDenied),
        ServiceScope::System,
        "fallback",
    );
    assert!(err.message().contains("sudo ocd setup"));
    let err = map_privilege(
        &std::io::Error::from(std::io::ErrorKind::AlreadyExists),
        ServiceScope::User,
        "fallback",
    );
    assert!(err.message().contains("refusing to overwrite"));
}

#[test]
fn setup_roots_production_system_defaults_without_config() {
    let (roots, path, scope) = SetupRoots::production(Path::new("/tmp"), None, true).unwrap();
    assert_eq!(scope, ServiceScope::System);
    assert_eq!(path, PathBuf::from("/etc/open-compute/config.toml"));
    assert_eq!(roots.config_parent, PathBuf::from("/etc/open-compute"));
    assert_eq!(roots.data_dir, PathBuf::from("/var/lib/open-compute"));
    assert_eq!(
        roots.secrets_dir,
        PathBuf::from("/var/lib/open-compute/secrets")
    );
}

#[test]
fn setup_roots_production_defaults_to_host_user_locations() {
    let home = PathBuf::from(std::env::var_os("HOME").unwrap());
    let (roots, path, scope) = SetupRoots::production(Path::new("/tmp"), None, false).unwrap();
    assert_eq!(scope, ServiceScope::User);
    #[cfg(target_os = "macos")]
    {
        assert_eq!(
            path,
            home.join("Library/Application Support/open-compute/config.toml")
        );
        assert_eq!(
            roots.data_dir,
            home.join("Library/Application Support/open-compute/data")
        );
    }
    #[cfg(not(target_os = "macos"))]
    {
        let expected_config = std::env::var_os("XDG_CONFIG_HOME")
            .filter(|value| !value.is_empty())
            .map_or_else(|| home.join(".config"), PathBuf::from)
            .join("open-compute/config.toml");
        let expected_data = std::env::var_os("XDG_DATA_HOME")
            .filter(|value| !value.is_empty())
            .map_or_else(|| home.join(".local/share"), PathBuf::from)
            .join("open-compute");
        assert_eq!(path, expected_config);
        assert_eq!(roots.data_dir, expected_data);
    }
}

#[test]
fn setup_roots_production_user_scope_with_config() {
    let temp = TempDir::new().unwrap();
    let config = temp.path().join("home/.config/open-compute/config.toml");
    let (roots, path, scope) = SetupRoots::production(temp.path(), Some(&config), false).unwrap();
    assert_eq!(scope, ServiceScope::User);
    assert_eq!(path, config);
    assert_eq!(roots.config_parent, config.parent().unwrap());
    assert!(roots.data_dir.ends_with(".local/share/open-compute") || roots.data_dir.is_absolute());
}

#[test]
fn setup_roots_rejects_system_combined_with_config() {
    let err =
        SetupRoots::production(Path::new("/tmp"), Some(Path::new("/x.toml")), true).unwrap_err();
    assert!(err.message().contains("cannot be combined"));
}

#[test]
fn interactive_system_scope_with_start_and_disabled_dashboard() {
    let temp = TempDir::new().unwrap();
    let (mut options, _) = test_options(&temp);
    options.config = None;
    options.scope = ServiceScope::System;
    let config = temp.path().join("etc/open-compute/config.toml");
    let data = temp.path().join("var/lib/open-compute");
    let input = format!(
        "{}\n{}\n127.0.0.1:9797\nlocal\nn\ny\n",
        config.display(),
        data.display()
    );
    let mut prompts = Vec::new();
    let plan = plan_interactive(
        &options,
        temp.path(),
        &mut Cursor::new(input.into_bytes()),
        &mut prompts,
    )
    .unwrap();
    assert_eq!(plan.scope, ServiceScope::System);
    assert!(!plan.dashboard_enabled);
    assert!(plan.start_service);
    assert_eq!(plan.public_bind, "127.0.0.1:9797");
    let fake = FakeServiceManager::default();
    let mut out = Vec::new();
    execute_plan(&plan, temp.path(), &fake, &mut out).unwrap();
    assert_eq!(fake.installed().len(), 1);
    assert_eq!(fake.started().len(), 1);
    let text = String::from_utf8(out).unwrap();
    assert!(text.contains("SETUP_OK"));
    assert!(text.contains("scope=system"));
    let loaded = load_platform_config_from(&plan.config_path, temp.path()).unwrap();
    assert!(!loaded.config.dashboard.enabled);
}

#[test]
fn interactive_user_defaults_with_start_service() {
    let temp = TempDir::new().unwrap();
    let (options, _) = test_options(&temp);
    let mut input = Cursor::new(b"\n\n\n\n\ny\n".to_vec());
    let plan = plan_interactive(&options, temp.path(), &mut input, &mut Vec::new()).unwrap();
    assert_eq!(plan.scope, ServiceScope::User);
    assert!(plan.start_service);
    let fake = FakeServiceManager::default();
    execute_plan(&plan, temp.path(), &fake, &mut Vec::new()).unwrap();
    assert!(!fake.installed().is_empty());
}
