use super::*;
use open_compute_core::clock::SystemClock;
use open_compute_core::{LocalObjectStorageConfig, PlatformConfig, S3Config};
use open_compute_storage::PlatformStorage;
use std::os::unix::fs::{PermissionsExt, symlink};

fn registry() -> (tempfile::TempDir, InstanceRegistry) {
    let temp = tempfile::tempdir().unwrap();
    let registry =
        InstanceRegistry::with_roots(temp.path().join("system"), temp.path().join("user"));
    (temp, registry)
}

#[test]
fn scoped_admin_reference_resolves_from_ocd_root() {
    let (_temp, registry) = registry();
    let root = registry.root_for(ServiceScope::User);
    fs::create_dir_all(root).unwrap();
    let manifest = root.join("ocd.toml");
    fs::write(
        &manifest,
        "[server]\nadmin_auth = { file = './keys/admin.token' }\n",
    )
    .unwrap();
    fs::set_permissions(&manifest, fs::Permissions::from_mode(0o600)).unwrap();
    let server = registry.server_config(ServiceScope::User).unwrap();
    let expected = root.join("keys/admin.token");
    assert_eq!(server.admin_auth.file.as_deref(), Some(expected.as_path()));
}

#[test]
fn shared_git_limit_is_manifest_owned_and_bounded() {
    let (_temp, registry) = registry();
    let root = registry.root_for(ServiceScope::User);
    fs::create_dir_all(root).unwrap();
    let manifest = root.join("ocd.toml");
    for (body, expected) in [
        ("[artifacts]\nmax_concurrent_requests = 1\n", Some(1)),
        ("[artifacts]\nmax_concurrent_requests = 1024\n", Some(1024)),
        ("[artifacts]\nmax_concurrent_requests = 0\n", None),
        ("[artifacts]\nmax_concurrent_requests = 1025\n", None),
    ] {
        fs::write(&manifest, body).unwrap();
        fs::set_permissions(&manifest, fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(
            registry
                .artifacts_config(ServiceScope::User)
                .ok()
                .map(|config| config.max_concurrent_requests),
            expected
        );
    }
}

#[test]
fn shared_metric_limit_is_manifest_owned_and_bounded() {
    let (_temp, registry) = registry();
    let root = registry.root_for(ServiceScope::User);
    fs::create_dir_all(root).unwrap();
    let manifest = root.join("ocd.toml");
    for (body, expected) in [
        ("[metrics]\nmax_series = 752\n", Some(752)),
        ("[metrics]\nmax_series = 1024\n", Some(1024)),
        ("[metrics]\nmax_series = 751\n", None),
        ("[metrics]\nmax_series = 0\n", None),
    ] {
        fs::write(&manifest, body).unwrap();
        fs::set_permissions(&manifest, fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(
            registry
                .metrics_config(ServiceScope::User)
                .ok()
                .map(|config| config.max_series),
            expected
        );
    }
}

#[test]
fn passwd_home_requires_matching_uid_and_absolute_safe_path() {
    assert_eq!(
        user_home::passwd_home("dev:x:501:20::0:0:Dev:/Users/dev:/bin/sh\n", 501).unwrap(),
        PathBuf::from("/Users/dev")
    );
    assert_eq!(
        user_home::passwd_home("dev:x:1001:1001:Dev:/home/dev:/bin/sh\n", 1001).unwrap(),
        PathBuf::from("/home/dev")
    );
    for record in [
        "dev:x:502:20::0:0:Dev:/Users/dev:/bin/sh\n",
        "dev:x:501:20::0:0:Dev:relative:/bin/sh\n",
        "dev:x:501:20::0:0:Dev:/Users/../root:/bin/sh\n",
        "dev:x:501:20::0:0:Dev:/Users/dev\n",
    ] {
        assert!(user_home::passwd_home(record, 501).is_err());
    }
}

#[test]
fn system_scope_uses_ocd_directory_owner_not_sudo_environment() {
    let (temp, registry) = registry();
    let root = registry.root_for(ServiceScope::System);
    fs::create_dir_all(root).unwrap();
    if rustix::process::getuid().is_root() {
        assert!(validate_scope_owner(ServiceScope::System, root).is_err());
    } else {
        validate_scope_owner(ServiceScope::System, root).unwrap();
    }
    let alias = temp.path().join("alias");
    symlink(root, &alias).unwrap();
    assert!(validate_scope_owner(ServiceScope::System, &alias).is_err());
}

fn initialized_config(root: &Path, config_path: &Path, data_path: &Path) -> PathBuf {
    fs::create_dir_all(config_path.parent().unwrap()).unwrap();
    fs::create_dir_all(data_path.join("keys")).unwrap();
    let key = data_path.join("keys/master.key");

    let mut config = PlatformConfig::local_test_config();
    config.data.path = data_path.to_owned();
    config.data.master_key_file = key;
    config.object_storage = ObjectStorageConfig::Local(LocalObjectStorageConfig {
        path: data_path.join("objects"),
        ..LocalObjectStorageConfig::default()
    });
    fs::write(config_path, toml::to_string_pretty(&config).unwrap()).unwrap();
    let canonical = config_path.canonicalize().unwrap();
    validate_instance_data_path(root, data_path).unwrap();
    drop(PlatformStorage::bootstrap(&config.data, &SystemClock).unwrap());
    canonical
}

#[test]
fn manifest_persists_only_config_and_autostart() {
    let (_temp, registry) = registry();
    let root = registry.root_for(ServiceScope::User);
    fs::create_dir_all(root.join("instances/dev")).unwrap();
    let config = initialized_config(
        root,
        &root.join("instances/dev/compute.toml"),
        &root.join("instances/dev/data"),
    );
    let record = registry
        .register(&config, ServiceScope::User, SystemTime::UNIX_EPOCH)
        .unwrap();
    let body = fs::read_to_string(root.join(MANIFEST_NAME)).unwrap();
    assert!(body.contains("[[instances]]"));
    assert!(body.contains("config ="));
    assert!(body.contains("autostart = true"));
    assert!(body.contains("[server]"));
    assert_eq!(
        registry
            .server_config(ServiceScope::User)
            .unwrap()
            .public_bind,
        "127.0.0.1:8787"
    );
    for forbidden in ["instance_id", "data_path", "digest", "binary", "service"] {
        assert!(!body.contains(forbidden), "{forbidden}: {body}");
    }
    assert_eq!(
        registry.list_scope(ServiceScope::User).unwrap(),
        vec![record.clone()]
    );
    assert_eq!(
        registry.scope_for_config(&config).unwrap(),
        ServiceScope::User
    );
    registry.remove_record(&record).unwrap();
    assert!(registry.list_scope(ServiceScope::User).unwrap().is_empty());
}

#[test]
fn restore_target_uses_configs_without_needing_the_missing_data_authority() {
    let (_temp, registry) = registry();
    let scope = ServiceScope::User;
    let root = registry.root_for(scope);
    let first_data = root.join("instances/first/data");
    let first = initialized_config(
        root,
        &root.join("instances/first/compute.toml"),
        &first_data,
    );
    registry
        .register(&first, scope, SystemTime::UNIX_EPOCH)
        .unwrap();
    let other_data = root.join("instances/other/data");
    let other = initialized_config(
        root,
        &root.join("instances/other/compute.toml"),
        &other_data,
    );
    let other_id = inspect_control_db(&other_data.join("control.sqlite"), 5_000)
        .unwrap()
        .1
        .instance_id;
    registry
        .register(&other, scope, SystemTime::UNIX_EPOCH)
        .unwrap();

    // Restoring a lost registered authority must not require opening its old DB.
    fs::remove_file(first_data.join("control.sqlite")).unwrap();
    assert_eq!(
        registry
            .validate_restore_target(scope, &first, &first_data)
            .unwrap(),
        vec![other_id]
    );

    let second_dir = root.join("instances/second");
    fs::create_dir_all(&second_dir).unwrap();
    let second = second_dir.join("compute.toml");
    let mut config = PlatformConfig::local_test_config();
    config.data.path = first_data.join("nested");
    config.data.master_key_file = config.data.path.join("keys/master.key");
    config.object_storage = ObjectStorageConfig::Local(LocalObjectStorageConfig {
        path: config.data.path.join("objects"),
        ..LocalObjectStorageConfig::default()
    });
    fs::write(&second, toml::to_string_pretty(&config).unwrap()).unwrap();
    let second = second.canonicalize().unwrap();
    assert_eq!(
        registry
            .validate_restore_target(scope, &second, &config.data.path)
            .unwrap_err()
            .code(),
        ErrorCode::InstanceRegistryInvalid,
    );
    assert_eq!(
        registry
            .validate_restore_target(scope, &second, &first_data)
            .unwrap_err()
            .code(),
        ErrorCode::InstanceRegistryInvalid,
    );
    config.data.path = root.join("instances/second/data");
    fs::write(&second, toml::to_string_pretty(&config).unwrap()).unwrap();
    assert_eq!(
        registry
            .validate_restore_target(scope, &second, &config.data.path)
            .unwrap_err()
            .code(),
        ErrorCode::InstanceNotFound
    );
}

#[test]
fn names_resolve_to_stored_ids_and_duplicates_fail_closed() {
    let (_temp, registry) = registry();
    let root = registry.root_for(ServiceScope::User);
    let first = initialized_config(
        root,
        &root.join("instances/first/compute.toml"),
        &root.join("instances/first/data"),
    );
    let mut config: PlatformConfig = toml::from_str(&fs::read_to_string(&first).unwrap()).unwrap();
    config.instance.name = Some("dev".parse().unwrap());
    fs::write(&first, toml::to_string(&config).unwrap()).unwrap();
    let record = registry
        .register(&first, ServiceScope::User, SystemTime::now())
        .unwrap();
    assert_eq!(record.name.as_deref(), Some("dev"));
    assert_eq!(
        registry
            .get_scope(ServiceScope::User, &"dev".parse().unwrap())
            .unwrap()
            .instance_id,
        record.instance_id
    );

    let second = initialized_config(
        root,
        &root.join("instances/second/compute.toml"),
        &root.join("instances/second/data"),
    );
    let mut config: PlatformConfig = toml::from_str(&fs::read_to_string(&second).unwrap()).unwrap();
    config.instance.name = Some("dev".parse().unwrap());
    fs::write(&second, toml::to_string(&config).unwrap()).unwrap();
    assert_eq!(
        registry
            .register(&second, ServiceScope::User, SystemTime::now())
            .unwrap_err()
            .code(),
        ErrorCode::InstanceRegistryInvalid
    );
    assert_eq!(registry.list_scope(ServiceScope::User).unwrap().len(), 1);
}

#[test]
fn registration_rejects_overlapping_public_base_domains() {
    let (_temp, registry) = registry();
    let root = registry.root_for(ServiceScope::User);
    let first = initialized_config(
        root,
        &root.join("instances/first/compute.toml"),
        &root.join("instances/first/data"),
    );
    let second = initialized_config(
        root,
        &root.join("instances/second/compute.toml"),
        &root.join("instances/second/data"),
    );
    let set_domain = |path: &Path, domain: &str| {
        let mut config: PlatformConfig =
            toml::from_str(&fs::read_to_string(path).unwrap()).unwrap();
        config.public_gateway = Some(open_compute_core::PublicDomainConfig {
            base_domain: domain.to_owned(),
        });
        fs::write(path, toml::to_string(&config).unwrap()).unwrap();
    };
    set_domain(&first, "example.com");
    registry
        .register(&first, ServiceScope::User, SystemTime::now())
        .unwrap();
    for domain in ["example.com", "shop.example.com"] {
        set_domain(&second, domain);
        assert_eq!(
            registry
                .register(&second, ServiceScope::User, SystemTime::now())
                .unwrap_err()
                .code(),
            ErrorCode::InstanceRegistryInvalid
        );
        assert_eq!(registry.list_scope(ServiceScope::User).unwrap().len(), 1);
    }
    set_domain(&second, "other.example.net");
    registry
        .register(&second, ServiceScope::User, SystemTime::now())
        .unwrap();
    assert_eq!(registry.list_scope(ServiceScope::User).unwrap().len(), 2);
}

#[test]
fn registration_rejects_overlapping_s3_prefixes_in_one_bucket() {
    let (_temp, registry) = registry();
    let root = registry.root_for(ServiceScope::User);
    let first = initialized_config(
        root,
        &root.join("instances/first/compute.toml"),
        &root.join("instances/first/data"),
    );
    let second = initialized_config(
        root,
        &root.join("instances/second/compute.toml"),
        &root.join("instances/second/data"),
    );
    let set_s3 = |path: &Path, prefix: &str, r2_prefix: &str| {
        let mut config: PlatformConfig =
            toml::from_str(&fs::read_to_string(path).unwrap()).unwrap();
        config.object_storage = ObjectStorageConfig::S3(S3Config {
            prefix: prefix.to_owned(),
            r2_prefix: r2_prefix.to_owned(),
            ..S3Config::default()
        });
        fs::write(path, toml::to_string(&config).unwrap()).unwrap();
    };
    set_s3(&first, "system/", "tenant/r2/");
    registry
        .register(&first, ServiceScope::User, SystemTime::now())
        .unwrap();
    for (prefix, r2_prefix) in [
        ("other-system/", "tenant/r2/"),
        ("tenant/r2/nested/", "other-r2/"),
    ] {
        set_s3(&second, prefix, r2_prefix);
        assert_eq!(
            registry
                .register(&second, ServiceScope::User, SystemTime::now())
                .unwrap_err()
                .code(),
            ErrorCode::InstanceRegistryInvalid
        );
    }
    set_s3(&second, "other-system/", "other-r2/");
    registry
        .register(&second, ServiceScope::User, SystemTime::now())
        .unwrap();
}

#[test]
fn manifest_preserves_shared_listener_when_registration_changes() {
    let (_temp, registry) = registry();
    let root = registry.root_for(ServiceScope::User);
    fs::create_dir_all(root.join("instances/dev")).unwrap();
    let manifest = root.join(MANIFEST_NAME);
    fs::write(
        &manifest,
        "[server]\npublic_bind = \"127.0.0.1:9191\"\nadmin_bind = \"127.0.0.1:9192\"\n",
    )
    .unwrap();
    fs::set_permissions(&manifest, fs::Permissions::from_mode(0o600)).unwrap();
    let config = initialized_config(
        root,
        &root.join("instances/dev/compute.toml"),
        &root.join("instances/dev/data"),
    );
    let record = registry
        .register(&config, ServiceScope::User, SystemTime::now())
        .unwrap();
    let server = registry.server_config(ServiceScope::User).unwrap();
    assert_eq!(server.public_addr().unwrap().to_string(), "127.0.0.1:9191");
    assert_eq!(
        server.admin_addr().unwrap().unwrap().to_string(),
        "127.0.0.1:9192"
    );
    registry.remove_record(&record).unwrap();
    assert_eq!(
        registry
            .server_config(ServiceScope::User)
            .unwrap()
            .public_bind,
        "127.0.0.1:9191"
    );
    fs::write(&manifest, "[server]\npublic_bind = \"not a socket\"\n").unwrap();
    assert!(registry.server_config(ServiceScope::User).is_err());
}

#[test]
fn instances_directory_is_never_scanned_or_used_as_identity() {
    let (_temp, registry) = registry();
    let root = registry.root_for(ServiceScope::User);
    fs::create_dir_all(root.join("instances/unregistered")).unwrap();
    let first = initialized_config(
        root,
        &root.join("instances/unregistered/compute.toml"),
        &root.join("instances/unregistered/data"),
    );
    assert!(registry.list_scope(ServiceScope::User).unwrap().is_empty());

    let first_record = registry
        .register(&first, ServiceScope::User, SystemTime::now())
        .unwrap();
    registry.remove_record(&first_record).unwrap();
    let moved = root.join("instances/renamed/compute.toml");
    fs::create_dir_all(moved.parent().unwrap()).unwrap();
    fs::copy(&first, &moved).unwrap();
    let moved = moved.canonicalize().unwrap();
    let moved_record = registry
        .register(&moved, ServiceScope::User, SystemTime::now())
        .unwrap();
    assert_eq!(moved_record.instance_id, first_record.instance_id);
}

#[test]
fn scope_lookup_uses_only_explicit_config_entries() {
    let (temp, registry) = registry();
    let config = temp.path().join("uninitialized.toml");
    fs::write(&config, "not a platform config").unwrap();
    let config = config.canonicalize().unwrap();
    let user = registry.root_for(ServiceScope::User);
    fs::create_dir_all(user).unwrap();
    let manifest = user.join(MANIFEST_NAME);
    fs::write(
        &manifest,
        "[[instances]]\nconfig = \"../uninitialized.toml\"\nautostart = true\n",
    )
    .unwrap();
    fs::set_permissions(&manifest, fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(
        registry.scope_for_config(&config).unwrap(),
        ServiceScope::User
    );
    assert_eq!(
        registry
            .scope_for_config(&temp.path().join("missing.toml"))
            .unwrap_err()
            .code(),
        ErrorCode::InstanceNotFound
    );
    let system = registry.root_for(ServiceScope::System);
    fs::create_dir_all(system).unwrap();
    let system_manifest = system.join(MANIFEST_NAME);
    fs::write(
        &system_manifest,
        format!("[[instances]]\nconfig = {config:?}\nautostart = false\n"),
    )
    .unwrap();
    fs::set_permissions(&system_manifest, fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(
        registry.scope_for_config(&config).unwrap_err().code(),
        ErrorCode::InstanceRegistryInvalid
    );
}

#[test]
fn registry_rejects_uninitialized_data_and_changed_storage_identity() {
    let (temp, registry) = registry();
    let root = registry.root_for(ServiceScope::User);
    let config = initialized_config(
        root,
        &root.join("instances/dev/compute.toml"),
        &root.join("instances/dev/data"),
    );
    let record = registry
        .register(&config, ServiceScope::User, SystemTime::now())
        .unwrap();
    let mut changed: PlatformConfig =
        toml::from_str(&fs::read_to_string(&config).unwrap()).unwrap();
    changed.data.path = temp.path().join("other-data");
    changed.data.master_key_file = changed.data.path.join("keys/master.key");
    changed.object_storage = ObjectStorageConfig::Local(LocalObjectStorageConfig {
        path: changed.data.path.join("objects"),
        ..LocalObjectStorageConfig::default()
    });
    fs::create_dir_all(changed.data.path.join("keys")).unwrap();
    fs::write(&config, toml::to_string_pretty(&changed).unwrap()).unwrap();
    assert_eq!(
        registry
            .validate_registered_config(&record)
            .unwrap_err()
            .code(),
        ErrorCode::InstanceRegistryInvalid
    );
    assert_eq!(
        registry.list_scope(ServiceScope::User).unwrap_err().code(),
        ErrorCode::InstanceRegistryInvalid
    );
    drop(PlatformStorage::bootstrap(&changed.data, &SystemClock).unwrap());
    assert_eq!(
        registry
            .validate_registered_config(&record)
            .unwrap_err()
            .code(),
        ErrorCode::InstanceRegistryInvalid
    );
}

#[test]
fn moving_registered_config_does_not_change_explicit_data_or_identity() {
    let (_temp, registry) = registry();
    let root = registry.root_for(ServiceScope::User);
    let data = root.join("instances/dev/data");
    let config = initialized_config(root, &root.join("instances/dev/compute.toml"), &data);
    let original = registry
        .register(&config, ServiceScope::User, SystemTime::now())
        .unwrap();
    let moved_parent = root.join("instances/config-only");
    fs::create_dir_all(&moved_parent).unwrap();
    let moved = moved_parent.join("compute.toml");
    fs::rename(&config, &moved).unwrap();
    let manifest_path = root.join("ocd.toml");
    let mut manifest: toml::Value =
        toml::from_str(&fs::read_to_string(&manifest_path).unwrap()).unwrap();
    manifest["instances"][0]["config"] = toml::Value::String(moved.display().to_string());
    fs::write(&manifest_path, toml::to_string_pretty(&manifest).unwrap()).unwrap();
    let relocated = registry.list_scope(ServiceScope::User).unwrap();
    assert_eq!(relocated.len(), 1);
    assert_eq!(relocated[0].instance_id, original.instance_id);
    assert_eq!(
        relocated[0].data_path,
        data.canonicalize().unwrap().display().to_string()
    );
    assert_eq!(
        relocated[0].canonical_config_path,
        moved.canonicalize().unwrap().display().to_string()
    );
    assert!(data.join("control.sqlite").exists());
    assert!(!config.exists());
}

#[test]
fn data_path_boundary_rejects_invalid_ancestors_and_relative_roots() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("ocd");
    fs::create_dir_all(root.join("instances")).unwrap();
    assert_eq!(
        validate_instance_data_path(Path::new("relative"), &root.join("instances/dev/data"))
            .unwrap_err()
            .code(),
        ErrorCode::PathInvalid
    );
    fs::write(root.join("instances/file"), b"not a directory").unwrap();
    assert_eq!(
        validate_instance_data_path(&root, &root.join("instances/file/data"))
            .unwrap_err()
            .code(),
        ErrorCode::PathInvalid
    );
    assert_eq!(
        validate_instance_data_path(&root, Path::new("/../../escape"))
            .unwrap_err()
            .code(),
        ErrorCode::PathInvalid
    );
}

#[test]
fn data_path_boundary_accepts_only_instances_descendants_or_external_roots() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("ocd");
    fs::create_dir_all(root.join("instances")).unwrap();
    let canonical_root = root.canonicalize().unwrap();
    let allowed = canonical_root.join("instances/dev/data");
    assert_eq!(
        validate_instance_data_path(&root, &allowed).unwrap(),
        allowed
    );
    let external = temp.path().canonicalize().unwrap().join("external/data");
    assert_eq!(
        validate_instance_data_path(&root, &external).unwrap(),
        external
    );
    for denied in [
        root.clone(),
        root.join("instances"),
        root.join("custom/data"),
        root.join("instances-old/data"),
        temp.path().to_owned(),
    ] {
        assert_eq!(
            validate_instance_data_path(&root, &denied)
                .unwrap_err()
                .code(),
            ErrorCode::PathInvalid,
            "{}",
            denied.display()
        );
    }
}

#[test]
fn data_path_boundary_rejects_symlink_escape() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("ocd");
    let external = temp.path().join("external");
    fs::create_dir_all(root.join("instances")).unwrap();
    fs::create_dir(&external).unwrap();
    fs::create_dir(external.join("data")).unwrap();
    symlink(&external, root.join("instances/link")).unwrap();
    assert_eq!(
        validate_instance_data_path(&root, &root.join("instances/link/missing"))
            .unwrap_err()
            .code(),
        ErrorCode::PathInvalid
    );
    assert_eq!(
        validate_instance_data_path(&root, &root.join("instances/link/data"))
            .unwrap_err()
            .code(),
        ErrorCode::PathInvalid
    );
    assert_eq!(
        validate_instance_data_path(&root, &root.join("instances/link/../safe/data"))
            .unwrap_err()
            .code(),
        ErrorCode::PathInvalid
    );
    fs::create_dir(root.join("instances/dev")).unwrap();
    assert_eq!(
        validate_instance_data_path(&root, &root.join("instances/dev/../safe/data")).unwrap(),
        root.canonicalize().unwrap().join("instances/safe/data")
    );
}

#[test]
fn malformed_or_permissive_manifest_fails_closed() {
    let (_temp, registry) = registry();
    let root = registry.root_for(ServiceScope::User);
    fs::create_dir_all(root).unwrap();
    let manifest = root.join(MANIFEST_NAME);
    fs::write(&manifest, "[[instances]]\nconfig = 1\nautostart = true\n").unwrap();
    fs::set_permissions(&manifest, fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(
        registry.list_scope(ServiceScope::User).unwrap_err().code(),
        ErrorCode::InstanceRegistryInvalid
    );
    fs::write(&manifest, "instances = []\nunknown = true\n").unwrap();
    assert_eq!(
        registry.list_scope(ServiceScope::User).unwrap_err().code(),
        ErrorCode::InstanceRegistryInvalid
    );
    fs::write(&manifest, "instances = []\n").unwrap();
    fs::set_permissions(&manifest, fs::Permissions::from_mode(0o644)).unwrap();
    assert_eq!(
        registry.list_scope(ServiceScope::User).unwrap_err().code(),
        ErrorCode::InstanceRegistryInvalid
    );
    fs::remove_file(&manifest).unwrap();
    let target = root.join("manifest-target");
    fs::write(&target, "instances = []\n").unwrap();
    symlink(&target, &manifest).unwrap();
    assert_eq!(
        registry.list_scope(ServiceScope::User).unwrap_err().code(),
        ErrorCode::InstanceRegistryInvalid
    );
    fs::remove_file(&manifest).unwrap();
    fs::write(&manifest, vec![b' '; MAX_MANIFEST_BYTES as usize + 1]).unwrap();
    fs::set_permissions(&manifest, fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(
        registry.list_scope(ServiceScope::User).unwrap_err().code(),
        ErrorCode::InstanceRegistryInvalid
    );
}

#[test]
fn duplicate_data_or_identity_is_rejected() {
    let (_temp, registry) = registry();
    let root = registry.root_for(ServiceScope::User);
    let data = root.join("instances/a/data");
    let first = initialized_config(root, &root.join("instances/a/compute.toml"), &data);
    let second = root.join("instances/b/compute.toml");
    fs::create_dir_all(second.parent().unwrap()).unwrap();
    fs::copy(&first, &second).unwrap();
    let first = first.to_string_lossy();
    let second = second.canonicalize().unwrap();
    let body = format!(
        "[[instances]]\nconfig = {first:?}\nautostart = true\n\n[[instances]]\nconfig = {:?}\nautostart = false\n",
        second.to_string_lossy()
    );
    ensure_ocd_root(root).unwrap();
    fs::write(root.join(MANIFEST_NAME), body).unwrap();
    fs::set_permissions(root.join(MANIFEST_NAME), fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(
        registry.list_scope(ServiceScope::User).unwrap_err().code(),
        ErrorCode::InstanceRegistryInvalid
    );
}

#[test]
fn duplicate_config_and_overlapping_data_are_rejected() {
    let (_temp, registry) = registry();
    let root = registry.root_for(ServiceScope::User);
    let first = initialized_config(
        root,
        &root.join("instances/a/compute.toml"),
        &root.join("instances/a/data"),
    );
    let first_text = first.to_string_lossy();
    ensure_ocd_root(root).unwrap();
    let manifest = root.join(MANIFEST_NAME);
    fs::write(
        &manifest,
        format!(
            "[[instances]]\nconfig = {first_text:?}\nautostart = true\n\n[[instances]]\nconfig = {first_text:?}\nautostart = false\n"
        ),
    )
    .unwrap();
    fs::set_permissions(&manifest, fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(
        registry.list_scope(ServiceScope::User).unwrap_err().code(),
        ErrorCode::InstanceRegistryInvalid
    );

    let second = initialized_config(
        root,
        &root.join("instances/b/compute.toml"),
        &root.join("instances/a/data/nested"),
    );
    fs::write(
        &manifest,
        format!(
            "[[instances]]\nconfig = {first_text:?}\nautostart = true\n\n[[instances]]\nconfig = {:?}\nautostart = false\n",
            second.to_string_lossy()
        ),
    )
    .unwrap();
    assert_eq!(
        registry.list_scope(ServiceScope::User).unwrap_err().code(),
        ErrorCode::InstanceRegistryInvalid
    );
}

#[test]
fn registration_requires_absolute_inputs_and_existing_authority() {
    let (temp, registry) = registry();
    assert_eq!(
        registry
            .register(
                Path::new("relative.toml"),
                ServiceScope::User,
                SystemTime::now()
            )
            .unwrap_err()
            .code(),
        ErrorCode::ConfigPathInvalid
    );
    assert_eq!(
        registry
            .register(
                &temp.path().join("missing.toml"),
                ServiceScope::User,
                SystemTime::now()
            )
            .unwrap_err()
            .code(),
        ErrorCode::InstanceRegistryInvalid
    );
    let root = registry.root_for(ServiceScope::User);
    let config = initialized_config(
        root,
        &root.join("instances/dev/compute.toml"),
        &root.join("instances/dev/data"),
    );
    let record = registry
        .register(&config, ServiceScope::User, SystemTime::now())
        .unwrap();
    let selector: InstanceSelector = record.instance_id.parse().unwrap();
    registry.remove_record(&record).unwrap();
    assert_eq!(
        registry
            .get_scope(ServiceScope::User, &selector)
            .unwrap_err()
            .code(),
        ErrorCode::InstanceNotFound
    );
    assert_eq!(
        registry
            .get_by_config_scope(ServiceScope::User, &config)
            .unwrap_err()
            .code(),
        ErrorCode::InstanceNotFound
    );
    fs::write(&config, "invalid = true").unwrap();
    let manifest = root.join(MANIFEST_NAME);
    fs::write(
        &manifest,
        format!("[[instances]]\nconfig = {config:?}\nautostart = true\n"),
    )
    .unwrap();
    assert_eq!(
        registry.list_scope(ServiceScope::User).unwrap_err().code(),
        ErrorCode::InstanceRegistryInvalid
    );
}

#[test]
fn service_scope_and_production_roots_are_stable() {
    assert_eq!(ServiceScope::System.as_str(), "system");
    assert_eq!(ServiceScope::User.as_str(), "user");
    let registry = InstanceRegistry::production().unwrap();
    assert_eq!(
        registry.root_for(ServiceScope::System),
        Path::new(SYSTEM_REGISTRY_ROOT)
    );
    assert!(registry.root_for(ServiceScope::User).is_absolute());
}
