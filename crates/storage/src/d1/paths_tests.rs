use super::*;

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
