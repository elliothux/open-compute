use super::*;
use crate::service_manager::FakeServiceManager;
use std::io::Cursor;
use std::os::unix::fs::PermissionsExt;
use tempfile::TempDir;

#[test]
fn system_setup_ownership_resolves_macos_root_alias_for_nested_instance_paths() {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("root");
    let alias = temp.path().join("alias");
    fs::create_dir(&root).unwrap();
    fs::create_dir_all(root.join("instances/dev/data")).unwrap();
    std::os::unix::fs::symlink(&root, &alias).unwrap();
    let canonical_root = root.canonicalize().unwrap();
    let service_user = crate::service_manager::SystemServiceUser {
        name: "test-user".to_owned(),
        uid: rustix::process::getuid().as_raw(),
        gid: rustix::process::getgid().as_raw(),
    };
    assert!(
        assign_scoped_ancestors(
            &alias,
            &canonical_root.join("instances/dev/compute.toml"),
            &service_user,
        )
        .unwrap()
    );
    assert!(
        assign_scoped_ancestors(
            &alias,
            &canonical_root.join("instances/dev/data"),
            &service_user,
        )
        .unwrap()
    );
    assert!(
        !assign_scoped_ancestors(
            &alias,
            &temp.path().join("external/compute.toml"),
            &service_user
        )
        .unwrap()
    );
}

fn test_options(temp: &TempDir) -> (SetupOptions, PathBuf) {
    let root = temp.path().to_path_buf();
    let config_parent = root.join("etc/open-compute");
    let data_dir = root.join("var/lib/open-compute");
    let secrets_dir = data_dir.join("keys");
    let system_registry = root.join("registry/system");
    let user_registry = root.join("registry/user");
    let config_path = config_parent.join("compute.toml");
    (
        SetupOptions {
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
    let global_keys = options.roots.user_registry_root.join("keys");
    fs::create_dir_all(&global_keys).unwrap();
    fs::write(global_keys.join("admin.token"), b"x\n").unwrap();
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
fn setup_rejects_secret_root_outside_explicit_data_before_writing() {
    let temp = TempDir::new().unwrap();
    let (mut options, config_path) = test_options(&temp);
    options.roots.secrets_dir = temp.path().join("other-keys");
    let error = run_setup(
        &options,
        temp.path(),
        &FakeServiceManager::default(),
        &mut Vec::new(),
    )
    .unwrap_err();
    assert_eq!(error.code(), ErrorCode::PathInvalid);
    assert!(!options.roots.secrets_dir.exists());
    assert!(!options.roots.data_dir.exists());
    assert!(!options.roots.user_registry_root.exists());
    assert!(!config_path.exists());
}

#[test]
fn fresh_setup_data_must_be_private_empty_directory() {
    let temp = TempDir::new().unwrap();
    let data = temp.path().join("data");
    fs::create_dir(&data).unwrap();
    fs::set_permissions(&data, fs::Permissions::from_mode(0o700)).unwrap();
    refuse_nonempty_data(&data).unwrap();
    fs::write(data.join("operator-file"), b"keep").unwrap();
    assert_eq!(
        refuse_nonempty_data(&data).unwrap_err().code(),
        ErrorCode::PathInvalid
    );
    assert_eq!(fs::read(data.join("operator-file")).unwrap(), b"keep");
    assert_eq!(
        refuse_nonempty_data(&data.join("operator-file"))
            .unwrap_err()
            .code(),
        ErrorCode::PathInvalid
    );
}

#[test]
fn pre_start_rollback_removes_registration_and_only_owned_publications() {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("ocd");
    let config = temp.path().join("compute.toml");
    let data = temp.path().join("data");
    create_instance(&root, ServiceScope::User, &config, &data, None).unwrap();
    let registry = InstanceRegistry::with_roots(temp.path().join("system"), root);
    let record = registry
        .register(&config, ServiceScope::User, SystemTime::now())
        .unwrap();
    let manager = FakeServiceManager::default();
    manager
        .install(ServiceScope::User, None, Path::new("/opt/ocd"))
        .unwrap();
    let published_path = temp.path().join("owned");
    let published =
        exclusive_write_bytes(&published_path, b"owned", 0o600, ServiceScope::User).unwrap();
    rollback_pre_start(&[published], Some(&record), true, &registry, &manager).unwrap();
    assert!(!published_path.exists());
    assert!(manager.installed().is_empty());
    assert!(registry.list_scope(ServiceScope::User).unwrap().is_empty());
    assert!(config.exists() && data.join("control.sqlite").exists());
}

#[test]
fn setup_persists_the_verified_real_data_path() {
    let temp = TempDir::new().unwrap();
    let (mut options, config_path) = test_options(&temp);
    let actual = temp.path().join("actual");
    let alias = temp.path().join("alias");
    fs::create_dir(&actual).unwrap();
    std::os::unix::fs::symlink(&actual, &alias).unwrap();
    options.roots.data_dir = alias.join("data");
    options.roots.secrets_dir = options.roots.data_dir.join("keys");
    run_setup(
        &options,
        temp.path(),
        &FakeServiceManager::default(),
        &mut Vec::new(),
    )
    .unwrap();
    let loaded = load_platform_config_from(&config_path, temp.path()).unwrap();
    assert_eq!(
        loaded.config.data.path,
        actual.canonicalize().unwrap().join("data")
    );
    assert!(loaded.config.data.path.join("control.sqlite").exists());
}

#[test]
fn setup_publication_failure_removes_only_its_published_secrets() {
    let temp = TempDir::new().unwrap();
    let (options, config_path) = test_options(&temp);
    let plan = plan_yes(&options);
    let staging = temp.path().join("staging");
    let staged_secrets = staging.join("secrets");
    fs::create_dir_all(&staged_secrets).unwrap();
    fs::create_dir_all(&plan.secrets_dir).unwrap();
    fs::create_dir_all(plan.user_registry_root.join("keys")).unwrap();
    fs::create_dir_all(config_path.parent().unwrap()).unwrap();
    for name in ["admin.token", "deployer.token", "read-only.token"] {
        fs::write(staged_secrets.join(name), name).unwrap();
    }
    fs::write(staging.join("compute.toml"), b"staged").unwrap();
    fs::write(&config_path, b"operator-owned").unwrap();

    let error = publish_exclusive(
        &plan,
        &staging,
        &staged_secrets,
        &plan.user_registry_root.join("keys/admin.token"),
        &plan.secrets_dir.join("deployer.token"),
        &plan.secrets_dir.join("read-only.token"),
    )
    .unwrap_err();
    assert_eq!(error.code(), ErrorCode::PathInvalid);
    assert_eq!(fs::read(&config_path).unwrap(), b"operator-owned");
    assert!(!plan.user_registry_root.join("keys/admin.token").exists());
    for name in ["deployer.token", "read-only.token"] {
        assert!(!plan.secrets_dir.join(name).exists());
    }
}

#[test]
fn setup_file_publication_enforces_mode_and_reports_incomplete_rollback() {
    let temp = TempDir::new().unwrap();
    let source = temp.path().join("staged");
    let target = temp.path().join("published");
    fs::write(&source, b"secret").unwrap();
    fs::set_permissions(&source, fs::Permissions::from_mode(0o644)).unwrap();
    let published = publish_file(&source, &target, ServiceScope::User).unwrap();
    assert_eq!(fs::read(&target).unwrap(), b"secret");
    assert_eq!(
        fs::metadata(&target).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(
        publish_file(&source, &target, ServiceScope::User)
            .unwrap_err()
            .code(),
        ErrorCode::PathInvalid
    );
    assert_eq!(
        publish_file(
            &source,
            &temp.path().join("missing/target"),
            ServiceScope::User
        )
        .unwrap_err()
        .code(),
        ErrorCode::PathInvalid
    );
    remove_published_files(&[published]).unwrap();
    assert!(!target.exists());
}

#[test]
fn setup_rollback_preserves_replaced_file_and_removes_other_owned_files() {
    let temp = TempDir::new().unwrap();
    let replaced = temp.path().join("replaced");
    let owned = temp.path().join("owned");
    let replaced_record =
        exclusive_write_bytes(&replaced, b"original", 0o600, ServiceScope::User).unwrap();
    let owned_record = exclusive_write_bytes(&owned, b"owned", 0o600, ServiceScope::User).unwrap();
    fs::rename(&replaced, temp.path().join("moved-original")).unwrap();
    fs::write(&replaced, b"replacement").unwrap();

    let error = remove_published_files(&[owned_record, replaced_record]).unwrap_err();
    assert_eq!(error.code(), ErrorCode::PathInvalid);
    assert_eq!(fs::read(&replaced).unwrap(), b"replacement");
    assert!(!owned.exists());
}

#[test]
fn setup_rejects_data_in_a_non_instances_ocd_subtree_before_writing() {
    let temp = TempDir::new().unwrap();
    let (options, config_path) = test_options(&temp);
    let mut plan = plan_yes(&options);
    plan.data_dir = options.roots.user_registry_root.join("custom/data");
    plan.secrets_dir = plan.data_dir.join("keys");
    let err = execute_plan(
        &plan,
        temp.path(),
        &FakeServiceManager::default(),
        &mut Vec::new(),
    )
    .unwrap_err();
    assert_eq!(err.code(), ErrorCode::PathInvalid);
    assert!(!config_path.exists());
    assert!(!plan.data_dir.exists());
}

#[test]
fn setup_yes_writes_secrets_and_loadable_config() {
    let temp = TempDir::new().unwrap();
    let (options, config_path) = test_options(&temp);
    let fake = FakeServiceManager::default();
    let mut out = Vec::new();
    run_setup(&options, temp.path(), &fake, &mut out).unwrap();

    let admin = options.roots.user_registry_root.join("keys/admin.token");
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

    let tmp_root = options.roots.user_registry_root.join("tmp");
    let scope_root = &options.roots.user_registry_root;
    let lock = scope_root.join("ocd.lock");
    assert_eq!(
        fs::metadata(&lock).unwrap().permissions().mode() & 0o777,
        0o600
    );
    crate::run::DaemonLock::acquire(scope_root).unwrap();
    assert_eq!(
        fs::metadata(&tmp_root).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(fs::read_dir(&tmp_root).unwrap().count(), 0);
    assert_eq!(
        fs::read_dir(config_path.parent().unwrap()).unwrap().count(),
        1
    );

    assert!(options.roots.data_dir.join("keys/master.key").exists());
    let loaded = load_platform_config_from(&config_path, temp.path()).unwrap();
    let registry = InstanceRegistry::with_roots(
        options.roots.system_registry_root.clone(),
        options.roots.user_registry_root.clone(),
    );
    let server = registry.server_config(ServiceScope::User).unwrap();
    assert_eq!(server.admin_auth.file.as_deref(), Some(admin.as_path()));
    assert!(server.admin_auth.env.is_none());
    assert!(loaded.config.dashboard.enabled);

    let text = String::from_utf8(out).unwrap();
    assert!(text.contains("SETUP_OK"));
    assert!(text.contains("ocd dashboard"));
    assert!(!fake.installed().is_empty());
    assert!(!fake.started().is_empty());
}

#[test]
fn ownership_transfer_accepts_only_the_fresh_bootstrap_layout() {
    let temp = TempDir::new().unwrap();
    let (options, _) = test_options(&temp);
    run_setup(
        &options,
        temp.path(),
        &FakeServiceManager::default(),
        &mut Vec::new(),
    )
    .unwrap();
    let service_user = crate::service_manager::SystemServiceUser {
        name: "test-user".to_owned(),
        uid: rustix::process::getuid().as_raw(),
        gid: rustix::process::getgid().as_raw(),
    };
    assign_initialized_data_ownership(&options.roots.data_dir, &service_user).unwrap();

    let unknown = options.roots.data_dir.join("unknown.txt");
    open_compute_storage::atomic_write(&unknown, b"retain this").unwrap();
    assert_eq!(
        assign_initialized_data_ownership(&options.roots.data_dir, &service_user)
            .unwrap_err()
            .code(),
        ErrorCode::PathInvalid
    );
    assert_eq!(fs::read(&unknown).unwrap(), b"retain this");

    fs::remove_file(&unknown).unwrap();
    let outside = temp.path().join("outside.txt");
    fs::write(&outside, b"outside").unwrap();
    std::os::unix::fs::symlink(&outside, options.roots.data_dir.join("control.sqlite-wal"))
        .unwrap();
    assert_eq!(
        assign_initialized_data_ownership(&options.roots.data_dir, &service_user)
            .unwrap_err()
            .code(),
        ErrorCode::PathInvalid
    );
    assert_eq!(fs::read(&outside).unwrap(), b"outside");
}

#[test]
fn setup_does_not_take_over_an_insecure_empty_data_directory() {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("data");
    fs::create_dir(&root).unwrap();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(
        refuse_nonempty_data(&root).unwrap_err().code(),
        ErrorCode::PathInvalid
    );
    assert_eq!(
        fs::metadata(&root).unwrap().permissions().mode() & 0o777,
        0o755
    );
}

#[test]
fn ownership_transfer_rejects_symlinks_and_invalid_user_ids() {
    let temp = TempDir::new().unwrap();
    let file = temp.path().join("known");
    fs::write(&file, b"keep").unwrap();
    let link = temp.path().join("link");
    std::os::unix::fs::symlink(&file, &link).unwrap();
    let mut service_user = crate::service_manager::SystemServiceUser {
        name: "test-user".to_owned(),
        uid: rustix::process::getuid().as_raw(),
        gid: rustix::process::getgid().as_raw(),
    };
    assert_eq!(
        assign_path_ownership(&link, &service_user)
            .unwrap_err()
            .code(),
        ErrorCode::PathInvalid
    );
    service_user.uid = u32::MAX;
    assert_eq!(
        assign_path_ownership(&file, &service_user)
            .unwrap_err()
            .code(),
        ErrorCode::PathInvalid
    );
    assert_eq!(fs::read(&file).unwrap(), b"keep");
}

#[test]
fn crashed_setup_staging_recovery_only_removes_verified_private_entries() {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("ocd");
    let _lock = crate::run::DaemonLock::acquire(&root).unwrap();
    let tmp = root.join("tmp");
    ensure_dir_secure(&tmp).unwrap();
    let stage = |id: Uuid| tmp.join(format!(".ocd-setup-staging-{id}"));
    let valid = stage(Uuid::now_v7());
    fs::create_dir(&valid).unwrap();
    fs::set_permissions(&valid, fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(
        valid.join(".owner"),
        b"open-compute setup staging\ncompute.toml\n",
    )
    .unwrap();
    fs::set_permissions(valid.join(".owner"), fs::Permissions::from_mode(0o600)).unwrap();
    fs::create_dir(valid.join("secrets")).unwrap();
    fs::set_permissions(valid.join("secrets"), fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(valid.join("secrets/admin.token"), b"secret").unwrap();
    fs::set_permissions(
        valid.join("secrets/admin.token"),
        fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    fs::write(valid.join("compute.toml"), b"config").unwrap();
    fs::set_permissions(
        valid.join("compute.toml"),
        fs::Permissions::from_mode(0o600),
    )
    .unwrap();

    let unmarked = stage(Uuid::now_v7());
    fs::create_dir(&unmarked).unwrap();
    fs::set_permissions(&unmarked, fs::Permissions::from_mode(0o700)).unwrap();
    let poisoned = stage(Uuid::now_v7());
    fs::create_dir(&poisoned).unwrap();
    fs::set_permissions(&poisoned, fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(
        poisoned.join(".owner"),
        b"open-compute setup staging\ncompute.toml\n",
    )
    .unwrap();
    fs::set_permissions(poisoned.join(".owner"), fs::Permissions::from_mode(0o600)).unwrap();
    fs::write(poisoned.join("unexpected"), b"retain").unwrap();
    fs::set_permissions(
        poisoned.join("unexpected"),
        fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    std::os::unix::fs::symlink(&valid, poisoned.join("secrets")).unwrap();
    let unknown = tmp.join("operator-notes");
    fs::write(&unknown, b"retain").unwrap();
    let linked = stage(Uuid::now_v7());
    std::os::unix::fs::symlink(&valid, &linked).unwrap();

    recover_setup_staging(&root).unwrap();
    assert!(!valid.exists());
    assert!(unmarked.exists());
    assert!(poisoned.exists());
    assert_eq!(fs::read(unknown).unwrap(), b"retain");
    assert!(
        fs::symlink_metadata(linked)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    fs::set_permissions(&tmp, fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(
        recover_setup_staging(&root).unwrap_err().code(),
        ErrorCode::PathInvalid
    );
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
    let listed = registry.list_scope(ServiceScope::User).unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].service_scope, ServiceScope::User);
}

#[test]
fn setup_install_failure_preserves_initialized_instance_for_repair() {
    let temp = TempDir::new().unwrap();
    let (options, config_path) = test_options(&temp);
    let fake = FakeServiceManager::default();
    fake.set_fail_install(true);
    let err = run_setup(&options, temp.path(), &fake, &mut Vec::new()).unwrap_err();
    assert_eq!(err.code(), ErrorCode::PlatformUnavailable);
    assert!(config_path.exists());
    assert!(
        options
            .roots
            .user_registry_root
            .join("keys/admin.token")
            .exists()
    );
    assert!(options.roots.secrets_dir.join("deployer.token").exists());
    assert!(options.roots.secrets_dir.join("read-only.token").exists());
    let registry = InstanceRegistry::with_roots(
        options.roots.system_registry_root.clone(),
        options.roots.user_registry_root.clone(),
    );
    assert_eq!(registry.list_scope(ServiceScope::User).unwrap().len(), 1);
    assert!(fake.installed().is_empty());
    assert!(fake.started().is_empty());
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
    assert_eq!(fake.installed(), vec![ServiceScope::User]);
    assert!(fake.started().is_empty());
    let registry = InstanceRegistry::with_roots(
        options.roots.system_registry_root.clone(),
        options.roots.user_registry_root.clone(),
    );
    assert_eq!(registry.list_scope(ServiceScope::User).unwrap().len(), 1);
    let text = String::from_utf8(out).unwrap();
    assert!(text.contains("SETUP_OK"));
    assert!(text.contains("started=false"));
}

#[test]
fn setup_refuses_nonempty_unknown_data_without_chown_or_mutation() {
    let temp = TempDir::new().unwrap();
    let (options, _) = test_options(&temp);
    fs::create_dir_all(&options.roots.data_dir).unwrap();
    let unknown = options.roots.data_dir.join("unowned");
    fs::write(&unknown, b"retain").unwrap();
    let error = run_setup(
        &options,
        temp.path(),
        &FakeServiceManager::default(),
        &mut Vec::new(),
    )
    .unwrap_err();
    assert_eq!(error.code(), ErrorCode::PathInvalid);
    assert_eq!(fs::read(unknown).unwrap(), b"retain");
    assert!(!options.config_path.exists());
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
    let (roots, path, scope) = SetupRoots::production(true).unwrap();
    assert_eq!(scope, ServiceScope::System);
    assert_eq!(
        path,
        PathBuf::from("/var/lib/open-compute/instances/default/compute.toml")
    );
    assert_eq!(
        roots.config_parent,
        PathBuf::from("/var/lib/open-compute/instances/default")
    );
    assert_eq!(
        roots.data_dir,
        PathBuf::from("/var/lib/open-compute/instances/default/data")
    );
    assert_eq!(
        roots.secrets_dir,
        PathBuf::from("/var/lib/open-compute/instances/default/data/keys")
    );
}

#[test]
fn setup_roots_production_defaults_to_host_user_locations() {
    let home = PathBuf::from(std::env::var_os("HOME").unwrap());
    let (roots, path, scope) = SetupRoots::production(false).unwrap();
    assert_eq!(scope, ServiceScope::User);
    let expected = home.join(".open-compute/instances/default");
    assert_eq!(path, expected.join("compute.toml"));
    assert_eq!(roots.data_dir, expected.join("data"));
}

#[test]
fn interactive_system_scope_with_start_and_disabled_dashboard() {
    let temp = TempDir::new().unwrap();
    let (mut options, _) = test_options(&temp);
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
    let registry = InstanceRegistry::with_roots(
        plan.system_registry_root.clone(),
        plan.user_registry_root.clone(),
    );
    assert_eq!(
        registry
            .server_config(ServiceScope::System)
            .unwrap()
            .public_bind,
        "127.0.0.1:9797"
    );
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
