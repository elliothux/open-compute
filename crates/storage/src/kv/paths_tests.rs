use super::*;
use std::fs::Permissions;
use std::os::unix::fs::PermissionsExt as _;
use std::os::unix::fs::symlink;

fn fixture() -> (tempfile::TempDir, KvPaths, AccountId, ResourceId) {
    let temp = tempfile::tempdir().unwrap();
    std::fs::set_permissions(temp.path(), Permissions::from_mode(0o700)).unwrap();
    let paths = KvPaths::open(temp.path()).unwrap();
    (temp, paths, AccountId::generate(), ResourceId::generate())
}

#[test]
fn typed_layout_publishes_quarantines_and_removes_exact_resource() {
    let (_temp, paths, account, resource) = fixture();
    assert_eq!(paths.root().file_name().unwrap(), "kv");
    assert_eq!(
        paths
            .resolve_storage_key("wrong", account, resource)
            .unwrap_err()
            .code(),
        ErrorCode::ResourceInvariantViolation
    );
    let expected = paths.database_path(account, resource);
    assert_eq!(
        paths
            .resolve_storage_key(&KvPaths::storage_key(account, resource), account, resource)
            .unwrap(),
        expected
    );

    let staging = paths.create_namespace_staging(resource).unwrap();
    std::fs::write(staging.join(DATABASE_FILE), b"db").unwrap();
    assert_eq!(
        paths.namespace_staging_candidates(resource).unwrap(),
        vec![staging.clone()]
    );
    paths.publish_staging(&staging, account, resource).unwrap();
    assert_eq!(std::fs::read(&expected).unwrap(), b"db");
    assert!(
        paths
            .publish_staging(
                &paths.create_namespace_staging(resource).unwrap(),
                account,
                resource
            )
            .is_err()
    );

    let quarantined = paths.quarantine(account, resource).unwrap().unwrap();
    assert_eq!(
        paths.quarantine_candidates(resource).unwrap(),
        vec![quarantined.clone()]
    );
    assert!(paths.quarantine(account, resource).unwrap().is_none());
    assert!(paths.remove_quarantine(paths.root()).is_err());
    paths.remove_quarantine(&quarantined).unwrap();
    assert!(!quarantined.exists());
}

#[test]
fn staging_candidates_and_write_staging_fail_closed() {
    let (_temp, paths, _account, resource) = fixture();
    let staging = paths.create_namespace_staging(resource).unwrap();
    std::fs::write(staging.join("partial"), b"x").unwrap();
    paths.remove_namespace_staging(&staging).unwrap();
    assert!(paths.remove_namespace_staging(paths.root()).is_err());

    std::fs::create_dir(paths.root().join(STAGING_DIR).join("unowned-shape")).unwrap();
    assert!(
        paths
            .namespace_staging_candidates(resource)
            .unwrap()
            .is_empty()
    );
    let request = uuid::Uuid::now_v7().hyphenated().to_string();
    let staged = paths.create_write_staging(resource, &request).unwrap();
    assert!(staged.is_file());
    assert!(paths.create_write_staging(resource, &request).is_err());
    assert!(paths.create_write_staging(resource, "invalid").is_err());
    assert_eq!(paths.cleanup_write_staging().unwrap(), 1);
    assert!(!staged.exists());

    let unknown = paths.root().join(WRITE_STAGING_DIR).join("unowned-shape");
    std::fs::create_dir(&unknown).unwrap();
    std::fs::write(unknown.join("payload"), b"keep").unwrap();
    assert_eq!(paths.cleanup_write_staging().unwrap(), 0);
    assert_eq!(std::fs::read(unknown.join("payload")).unwrap(), b"keep");
}

#[test]
fn removal_rejects_nested_or_symlinked_unknown_entries() {
    let (_temp, paths, _account, resource) = fixture();
    let staging = paths.create_namespace_staging(resource).unwrap();
    std::fs::create_dir(staging.join("nested")).unwrap();
    assert_eq!(
        paths.remove_namespace_staging(&staging).unwrap_err().code(),
        ErrorCode::ResourceInvariantViolation
    );
}

#[test]
fn cleanup_preserves_unknown_names_and_rejects_typed_symlinks() {
    let (_temp, paths, _account, resource) = fixture();
    let write_root = paths.root().join(WRITE_STAGING_DIR);
    let resource_dir = write_root.join(resource.to_string());
    std::fs::create_dir(&resource_dir).unwrap();
    std::fs::write(resource_dir.join("unknown-request"), b"keep").unwrap();
    assert_eq!(paths.cleanup_write_staging().unwrap(), 0);
    assert!(resource_dir.join("unknown-request").is_file());

    let target = paths.root().join("target");
    std::fs::create_dir(&target).unwrap();
    let candidate = paths
        .root()
        .join(STAGING_DIR)
        .join(format!("{resource}.{}", uuid::Uuid::now_v7().hyphenated()));
    symlink(&target, &candidate).unwrap();
    assert_eq!(
        paths
            .namespace_staging_candidates(resource)
            .unwrap_err()
            .code(),
        ErrorCode::ResourceInvariantViolation
    );
}

#[test]
fn quarantine_removal_rejects_nested_content() {
    let (_temp, paths, account, resource) = fixture();
    paths.ensure_account_dir(account).unwrap();
    let live = paths.namespace_dir(account, resource);
    std::fs::create_dir(&live).unwrap();
    std::fs::write(live.join(DATABASE_FILE), b"db").unwrap();
    let quarantine = paths.quarantine(account, resource).unwrap().unwrap();
    std::fs::create_dir(quarantine.join("nested")).unwrap();
    assert_eq!(
        paths.remove_quarantine(&quarantine).unwrap_err().code(),
        ErrorCode::ResourceInvariantViolation
    );
}
