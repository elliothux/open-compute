use super::*;
use std::os::unix::fs::symlink;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, UNIX_EPOCH};
use uuid::Uuid;

fn scratch_registry() -> (PathBuf, InstanceRegistry) {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    // Unique top-level directory: shared parents under TMPDIR fail Gate cleanup.
    let dir = std::env::temp_dir().join(format!(
        "open-compute-instance-registry-{}-{}",
        Uuid::now_v7().as_hyphenated(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&dir).unwrap();
    let system = dir.join("system");
    let user = dir.join("user");
    (dir, InstanceRegistry::with_roots(system, user))
}

fn write_valid_config(root: &Path, name: &str) -> PathBuf {
    let path = root.join(name);
    let config = open_compute_core::PlatformConfig::local_test_config();
    fs::write(&path, toml::to_string_pretty(&config).unwrap()).unwrap();
    path
}

#[test]
fn register_list_get_and_remove_round_trip() {
    let (dir, registry) = scratch_registry();
    let config = write_valid_config(&dir, "compute.toml");
    let canonical = config.canonicalize().unwrap();
    let record = registry
        .register(&canonical, ServiceScope::User, SystemTime::UNIX_EPOCH)
        .unwrap();
    assert_eq!(record.schema_version, REGISTRY_SCHEMA_VERSION);
    assert_eq!(record.canonical_config_path, canonical.to_string_lossy());
    let listed = registry.list().unwrap();
    assert_eq!(listed.len(), 1);
    let selector: InstanceSelector = record.instance_id.parse().unwrap();
    assert_eq!(registry.get(&selector).unwrap(), record);
    registry.remove(&selector).unwrap();
    assert!(registry.list().unwrap().is_empty());
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn system_registry_stays_service_readable_without_transferring_write_access() {
    let (dir, registry) = scratch_registry();
    let config = write_valid_config(&dir, "system.toml");
    let canonical = config.canonicalize().unwrap();
    let record = registry
        .register_with_service_user(
            &canonical,
            ServiceScope::System,
            Some("ocd-service"),
            SystemTime::UNIX_EPOCH,
        )
        .unwrap();
    let root_mode = fs::metadata(registry.root_for(ServiceScope::System))
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    let record_mode = fs::metadata(registry.record_path(ServiceScope::System, &record.instance_id))
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(root_mode, 0o755);
    assert_eq!(record_mode, 0o644);
    assert_eq!(registry.list().unwrap(), vec![record]);
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn collision_extends_short_id_without_renaming_existing() {
    let (dir, _registry) = scratch_registry();
    let path = dir.join("compute.toml");
    fs::write(&path, "x = 1\n").unwrap();
    let canonical = path.canonicalize().unwrap();
    let base = InstanceId::from_canonical_config_path(&canonical).unwrap();
    let occupied = vec![(base.as_str().to_owned(), [9u8; 32])];
    let allocated = allocate_instance_id(&canonical, &occupied).unwrap();
    assert_eq!(allocated.digest(), base.digest());
    assert_eq!(allocated.len(), base.len() + 1);
    assert!(allocated.as_str().starts_with(base.as_str()));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn symlink_registry_root_fails_closed() {
    let (dir, registry) = scratch_registry();
    let target = dir.join("target");
    fs::create_dir(&target).unwrap();
    symlink(&target, &registry.user_root).unwrap();
    let err = registry.list().unwrap_err();
    assert_eq!(err.code(), ErrorCode::InstanceRegistryInvalid);
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn unknown_schema_fails_closed() {
    let (dir, registry) = scratch_registry();
    ensure_registry_tree(registry.root_for(ServiceScope::User)).unwrap();
    let path = registry.record_path(ServiceScope::User, "abcde");
    fs::write(
        &path,
        br#"{"schema_version":99,"instance_id":"abcde","digest_sha256":"00","canonical_config_path":"/x","service_scope":"user","service_identifier":"x","created_at":0}"#,
    )
    .unwrap();
    let err = registry.list().unwrap_err();
    assert_eq!(err.code(), ErrorCode::InstanceRegistryInvalid);
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn register_rejects_relative_path_and_get_misses() {
    let (dir, registry) = scratch_registry();
    let err = registry
        .register(
            Path::new("relative.toml"),
            ServiceScope::User,
            SystemTime::UNIX_EPOCH,
        )
        .unwrap_err();
    assert_eq!(err.code(), ErrorCode::ConfigPathInvalid);
    let selector: InstanceSelector = "zzzzz".parse().unwrap();
    let err = registry.get(&selector).unwrap_err();
    assert_eq!(err.code(), ErrorCode::InstanceNotFound);
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn world_writable_root_fails_closed() {
    let (dir, registry) = scratch_registry();
    ensure_registry_tree(registry.root_for(ServiceScope::User)).unwrap();
    let root = registry.root_for(ServiceScope::User);
    fs::set_permissions(root, fs::Permissions::from_mode(0o777)).unwrap();
    let err = registry.list().unwrap_err();
    assert_eq!(err.code(), ErrorCode::InstanceRegistryInvalid);
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn production_registry_constructs() {
    let registry = InstanceRegistry::production().unwrap();
    assert!(registry.root_for(ServiceScope::System).is_absolute());
    assert!(registry.root_for(ServiceScope::User).is_absolute());
}

#[test]
fn instance_record_digest_mismatch_fails() {
    let record = InstanceRecord {
        schema_version: REGISTRY_SCHEMA_VERSION,
        instance_id: "abcde".to_owned(),
        digest_sha256: "11".repeat(32),
        canonical_config_path: "/tmp/no-such-config-for-digest.toml".to_owned(),
        config_sha256: "22".repeat(32),
        data_path: "/var/lib/open-compute".to_owned(),
        object_authority: RegisteredObjectAuthority::Local {
            path: "/var/lib/open-compute/objects".to_owned(),
        },
        binary_path: "/usr/local/bin/ocd".to_owned(),
        service_scope: ServiceScope::User,
        service_user: None,
        service_identifier: "dev.open-compute.ocd.abcde".to_owned(),
        created_at: 0,
    };
    assert!(record.instance_id().is_err());
}

#[test]
fn list_rejects_scope_mismatch_and_name_mismatch() {
    let (dir, registry) = scratch_registry();
    ensure_registry_tree(registry.root_for(ServiceScope::User)).unwrap();
    let path = registry.record_path(ServiceScope::User, "abcde");
    fs::write(
        &path,
        br#"{"schema_version":1,"instance_id":"abcde","digest_sha256":"00","canonical_config_path":"/x","service_scope":"system","service_identifier":"x","created_at":0}"#,
    )
    .unwrap();
    let err = registry.list().unwrap_err();
    assert_eq!(err.code(), ErrorCode::InstanceRegistryInvalid);
    fs::remove_file(&path).unwrap();
    fs::write(
        &path,
        br#"{"schema_version":1,"instance_id":"zzzzz","digest_sha256":"00","canonical_config_path":"/x","service_scope":"user","service_identifier":"x","created_at":0}"#,
    )
    .unwrap();
    let err = registry.list().unwrap_err();
    assert_eq!(err.code(), ErrorCode::InstanceRegistryInvalid);
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn list_rejects_non_directory_root_and_corrupt_json() {
    let (dir, registry) = scratch_registry();
    let root = registry.root_for(ServiceScope::User);
    fs::create_dir_all(root.parent().unwrap()).unwrap();
    fs::write(root, b"not-a-dir").unwrap();
    let err = registry.list().unwrap_err();
    assert_eq!(err.code(), ErrorCode::InstanceRegistryInvalid);
    fs::remove_file(root).unwrap();
    ensure_registry_tree(root).unwrap();
    let path = registry.record_path(ServiceScope::User, "abcde");
    fs::write(&path, b"{nope").unwrap();
    let err = registry.list().unwrap_err();
    assert_eq!(err.code(), ErrorCode::InstanceRegistryInvalid);
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn service_scope_as_str() {
    assert_eq!(ServiceScope::System.as_str(), "system");
    assert_eq!(ServiceScope::User.as_str(), "user");
}

#[test]
fn instance_id_rejects_path_digest_mismatch() {
    let dir = scratch_registry().0;
    let path_a = dir.join("a.toml");
    let path_b = dir.join("b.toml");
    fs::write(&path_a, "x = 1\n").unwrap();
    fs::write(&path_b, "x = 1\n").unwrap();
    let canonical_a = path_a.canonicalize().unwrap();
    let canonical_b = path_b.canonicalize().unwrap();
    let id = InstanceId::from_canonical_config_path(&canonical_a).unwrap();
    let record = InstanceRecord {
        schema_version: REGISTRY_SCHEMA_VERSION,
        instance_id: id.as_str().to_owned(),
        digest_sha256: hex::encode(id.digest()),
        canonical_config_path: canonical_b.to_string_lossy().into_owned(),
        config_sha256: "22".repeat(32),
        data_path: "/var/lib/open-compute".to_owned(),
        object_authority: RegisteredObjectAuthority::Local {
            path: "/var/lib/open-compute/objects".to_owned(),
        },
        binary_path: "/usr/local/bin/ocd".to_owned(),
        service_scope: ServiceScope::User,
        service_user: None,
        service_identifier: format!("dev.open-compute.ocd.{}", id.as_str()),
        created_at: 0,
    };
    let err = record.instance_id().unwrap_err();
    assert_eq!(err.code(), ErrorCode::InstanceRegistryInvalid);
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn get_rejects_duplicate_short_ids_across_scopes() {
    let (dir, registry) = scratch_registry();
    ensure_registry_tree(registry.root_for(ServiceScope::System)).unwrap();
    fs::set_permissions(
        registry.root_for(ServiceScope::System),
        fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    let config = write_valid_config(&dir, "compute.toml");
    let canonical = config.canonicalize().unwrap();
    let record = registry
        .register(&canonical, ServiceScope::User, SystemTime::UNIX_EPOCH)
        .unwrap();
    let dup_path = registry.record_path(ServiceScope::System, &record.instance_id);
    let mut dup = record.clone();
    dup.service_scope = ServiceScope::System;
    dup.service_user = Some("ocd-service".to_owned());
    dup.service_identifier = format!("dev.open-compute.ocd.{}", record.instance_id);
    fs::write(&dup_path, serde_json::to_vec_pretty(&dup).unwrap()).unwrap();
    fs::set_permissions(&dup_path, fs::Permissions::from_mode(0o644)).unwrap();
    let selector: InstanceSelector = record.instance_id.parse().unwrap();
    let err = registry.get(&selector).unwrap_err();
    assert_eq!(err.code(), ErrorCode::InstanceRegistryInvalid);
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn register_rejects_pre_epoch_clock() {
    let (dir, registry) = scratch_registry();
    let config = write_valid_config(&dir, "compute.toml");
    let canonical = config.canonicalize().unwrap();
    let err = registry
        .register(
            &canonical,
            ServiceScope::User,
            SystemTime::UNIX_EPOCH
                .checked_sub(Duration::from_secs(1))
                .unwrap(),
        )
        .unwrap_err();
    assert_eq!(err.code(), ErrorCode::InstanceRegistryInvalid);
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn write_record_refuses_overwrite() {
    let (dir, registry) = scratch_registry();
    let config = write_valid_config(&dir, "compute.toml");
    let canonical = config.canonicalize().unwrap();
    let record = registry
        .register(&canonical, ServiceScope::User, SystemTime::UNIX_EPOCH)
        .unwrap();
    let err = registry.write_record(&record).unwrap_err();
    assert_eq!(err.code(), ErrorCode::InstanceRegistryInvalid);
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn list_rejects_symlink_dir_and_world_writable_entries() {
    let (dir, registry) = scratch_registry();
    ensure_registry_tree(registry.root_for(ServiceScope::User)).unwrap();
    let root = registry.root_for(ServiceScope::User);

    let link = root.join("abcde.json");
    let target = dir.join("target.json");
    fs::write(&target, b"{}").unwrap();
    symlink(&target, &link).unwrap();
    let err = registry.list().unwrap_err();
    assert_eq!(err.code(), ErrorCode::InstanceRegistryInvalid);
    fs::remove_file(&link).unwrap();

    fs::create_dir(root.join("abcde.json")).unwrap();
    let err = registry.list().unwrap_err();
    assert_eq!(err.code(), ErrorCode::InstanceRegistryInvalid);
    fs::remove_dir(root.join("abcde.json")).unwrap();

    let config = write_valid_config(&dir, "compute.toml");
    let canonical = config.canonicalize().unwrap();
    let record = registry
        .register(&canonical, ServiceScope::User, SystemTime::UNIX_EPOCH)
        .unwrap();
    let path = registry.record_path(ServiceScope::User, &record.instance_id);
    fs::set_permissions(&path, fs::Permissions::from_mode(0o666)).unwrap();
    let err = registry.list().unwrap_err();
    assert_eq!(err.code(), ErrorCode::InstanceRegistryInvalid);
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn decode_digest_rejects_wrong_length() {
    let err = decode_digest("abcd").unwrap_err();
    assert_eq!(err.code(), ErrorCode::InstanceRegistryInvalid);
    let err = decode_digest("zz").unwrap_err();
    assert_eq!(err.code(), ErrorCode::InstanceRegistryInvalid);
}

#[test]
fn registration_enforces_scope_service_account_contract() {
    let (root, registry) = scratch_registry();
    let config = write_valid_config(&root, "config.toml");
    let canonical = config.canonicalize().unwrap();

    for service_user in [None, Some(""), Some("root")] {
        let err = registry
            .register_with_service_user(
                &canonical,
                ServiceScope::System,
                service_user,
                UNIX_EPOCH + Duration::from_secs(1),
            )
            .unwrap_err();
        assert_eq!(err.code(), ErrorCode::InstanceRegistryInvalid);
    }
    let err = registry
        .register_with_service_user(
            &canonical,
            ServiceScope::User,
            Some("unexpected"),
            UNIX_EPOCH + Duration::from_secs(1),
        )
        .unwrap_err();
    assert_eq!(err.code(), ErrorCode::InstanceRegistryInvalid);

    let record = registry
        .register_with_service_user(
            &canonical,
            ServiceScope::System,
            Some("ocd-service"),
            UNIX_EPOCH + Duration::from_secs(1),
        )
        .unwrap();
    let repeated = registry
        .register_with_service_user(
            &canonical,
            ServiceScope::System,
            Some("ocd-service"),
            UNIX_EPOCH + Duration::from_secs(2),
        )
        .unwrap();
    assert_eq!(repeated, record);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn system_registry_rejects_service_unreadable_root_and_record() {
    let (root, registry) = scratch_registry();
    let system_root = registry.root_for(ServiceScope::System);
    fs::create_dir_all(system_root).unwrap();
    fs::set_permissions(system_root, fs::Permissions::from_mode(0o700)).unwrap();
    let err = registry.list().unwrap_err();
    assert_eq!(err.code(), ErrorCode::InstanceRegistryInvalid);
    assert!(err.message().contains("readable by managed services"));

    fs::set_permissions(system_root, fs::Permissions::from_mode(0o755)).unwrap();
    let config = write_valid_config(&root, "config.toml");
    let record = registry
        .register_with_service_user(
            &config.canonicalize().unwrap(),
            ServiceScope::System,
            Some("ocd-service"),
            UNIX_EPOCH + Duration::from_secs(1),
        )
        .unwrap();
    let path = system_root.join(format!("{}.json", record.instance_id));
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    let err = registry.list().unwrap_err();
    assert_eq!(err.code(), ErrorCode::InstanceRegistryInvalid);
    assert!(err.message().contains("entry must be readable"));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn registry_rejects_service_account_drift_in_persisted_record() {
    let (root, registry) = scratch_registry();
    let config = write_valid_config(&root, "config.toml");
    let record = registry
        .register_with_service_user(
            &config.canonicalize().unwrap(),
            ServiceScope::System,
            Some("ocd-service"),
            UNIX_EPOCH + Duration::from_secs(1),
        )
        .unwrap();
    let path = registry
        .root_for(ServiceScope::System)
        .join(format!("{}.json", record.instance_id));
    let mut value: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    value["service_user"] = serde_json::Value::String("root".to_owned());
    fs::write(&path, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    let err = registry.list().unwrap_err();
    assert_eq!(err.code(), ErrorCode::InstanceRegistryInvalid);
    assert!(err.message().contains("service account"));
    let _ = fs::remove_dir_all(root);
}
