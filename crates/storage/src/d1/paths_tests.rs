use super::*;
use std::os::unix::fs::PermissionsExt as _;

#[test]
fn typed_paths_reject_wrong_parents_content_and_operation_symlinks() {
    let temp = tempfile::tempdir().unwrap();
    let paths = D1Paths::open(temp.path()).unwrap();
    let account = AccountId::generate();
    let resource = ResourceId::generate();
    assert_eq!(paths.root(), temp.path().join("d1"));
    assert_eq!(
        paths.database_path(account, resource),
        paths.database_dir(account, resource).join("data.sqlite")
    );
    assert_eq!(
        paths
            .resolve_storage_key("v1/wrong", account, resource)
            .unwrap_err()
            .code(),
        ErrorCode::D1IdentityMismatch
    );
    assert!(paths.quarantine(account, resource).unwrap().is_none());

    let outside = temp.path().join("outside");
    std::fs::create_dir(&outside).unwrap();
    assert_eq!(
        paths
            .publish_staging(&outside, account, resource)
            .unwrap_err()
            .code(),
        ErrorCode::D1IdentityMismatch
    );
    assert_eq!(
        paths.remove_operation_dir(&outside).unwrap_err().code(),
        ErrorCode::D1IdentityMismatch
    );

    let staging = paths.create_database_staging(resource).unwrap();
    std::fs::write(staging.join("unexpected"), b"x").unwrap();
    assert_eq!(
        paths.remove_operation_dir(&staging).unwrap_err().code(),
        ErrorCode::D1IdentityMismatch
    );
    std::fs::remove_file(staging.join("unexpected")).unwrap();
    std::fs::write(staging.join("data.sqlite"), b"x").unwrap();
    paths.publish_staging(&staging, account, resource).unwrap();
    let snapshot_key = D1Paths::snapshot_key(account, resource, 0);
    let snapshot_staging = paths.snapshot_staging_path(account, resource, 0).unwrap();
    std::fs::write(&snapshot_staging, b"snapshot").unwrap();
    std::fs::set_permissions(&snapshot_staging, std::fs::Permissions::from_mode(0o600)).unwrap();
    let snapshot = paths
        .publish_snapshot(&snapshot_staging, account, resource, 0)
        .unwrap();
    assert_eq!(
        paths
            .resolve_snapshot_key(&snapshot_key, account, resource, 0)
            .unwrap(),
        snapshot
    );
    assert_eq!(
        paths
            .resolve_snapshot_key("v1/wrong", account, resource, 0)
            .unwrap_err()
            .code(),
        ErrorCode::D1IdentityMismatch
    );
    let transfer_id = uuid::Uuid::now_v7().hyphenated().to_string();
    let transfer_key = paths
        .write_transfer(account, resource, &transfer_id, "expired.sql", b"SELECT 1;")
        .unwrap();
    let transfer_dir = paths
        .root()
        .join(".transfers")
        .join(account.to_string())
        .join(resource.to_string())
        .join(&transfer_id);
    paths
        .remove_pruned_transfer(
            &transfer_key,
            account,
            resource,
            &transfer_id,
            "expired.sql",
        )
        .unwrap();
    assert!(!transfer_dir.exists());
    let wrong_snapshot = paths.database_dir(account, resource).join("wrong.sqlite");
    std::fs::write(&wrong_snapshot, b"snapshot").unwrap();
    assert_eq!(
        paths
            .publish_snapshot(&wrong_snapshot, account, resource, 1)
            .unwrap_err()
            .code(),
        ErrorCode::D1IdentityMismatch
    );
    std::fs::remove_file(wrong_snapshot).unwrap();
    let replacement = paths.create_database_staging(resource).unwrap();
    assert_eq!(
        paths
            .publish_staging(&replacement, account, resource)
            .unwrap_err()
            .code(),
        ErrorCode::D1IdentityMismatch
    );
    paths.remove_operation_dir(&replacement).unwrap();
    let trash = paths.quarantine(account, resource).unwrap().unwrap();
    assert_eq!(
        paths.quarantine_candidates(resource).unwrap(),
        vec![trash.clone()]
    );
    paths.remove_operation_dir(&trash).unwrap();

    let ignored = paths.root().join(".staging").join("not-canonical");
    std::fs::create_dir(&ignored).unwrap();
    assert!(paths.staging_candidates(resource).unwrap().is_empty());
    std::fs::remove_dir(&ignored).unwrap();
    let linked = paths
        .root()
        .join(".staging")
        .join(format!("{resource}.{}", uuid::Uuid::now_v7().hyphenated()));
    std::os::unix::fs::symlink(temp.path(), &linked).unwrap();
    assert_eq!(
        paths.staging_candidates(resource).unwrap_err().code(),
        ErrorCode::D1IdentityMismatch
    );
}
