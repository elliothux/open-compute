use super::*;

#[test]
fn p1_restore_cleanup_rejects_ambiguous_bounds_links_receipts_and_lock_owners() {
    let (tmp, _) = unique_root();
    let parent = fs::canonicalize(tmp.path()).unwrap();
    let target = parent.join("cleanup-target");
    let make_staging = |id: &str| {
        let staging = parent.join(format!(".cleanup-target.restore-{id}"));
        fs::create_dir(&staging).unwrap();
        fs::set_permissions(&staging, fs::Permissions::from_mode(0o700)).unwrap();
        staging
    };

    let valid_id = uuid::Uuid::now_v7().hyphenated().to_string();
    for result in [
        crate::cleanup_restore_staging(&target, "not-a-uuid", 1, 1, 1),
        crate::cleanup_restore_staging(&target, &valid_id, 0, 1, 1),
        crate::cleanup_restore_staging(&target, &valid_id, 1, 0, 1),
        crate::cleanup_restore_staging(&target, &valid_id, 1, 2, 1),
        crate::cleanup_restore_staging(Path::new("relative"), &valid_id, 1, 1, 1),
    ] {
        assert!(result.is_err());
    }

    let empty_id = uuid::Uuid::now_v7().hyphenated().to_string();
    make_staging(&empty_id);
    let empty = crate::cleanup_restore_staging(&target, &empty_id, 1, 1, 1).unwrap();
    assert_eq!(empty.files, 0);
    assert_eq!(empty.bytes, 0);

    let size_id = uuid::Uuid::now_v7().hyphenated().to_string();
    let size_staging = make_staging(&size_id);
    fs::write(size_staging.join("control.sqlite"), b"large").unwrap();
    fs::set_permissions(
        size_staging.join("control.sqlite"),
        fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    assert!(crate::cleanup_restore_staging(&target, &size_id, 1, 4, 4).is_err());

    let count_id = uuid::Uuid::now_v7().hyphenated().to_string();
    let count_staging = make_staging(&count_id);
    for name in ["control.sqlite", "scheduler.sqlite"] {
        fs::write(count_staging.join(name), b"x").unwrap();
        fs::set_permissions(count_staging.join(name), fs::Permissions::from_mode(0o600)).unwrap();
    }
    assert!(crate::cleanup_restore_staging(&target, &count_id, 1, 1, 2).is_err());

    let total_id = uuid::Uuid::now_v7().hyphenated().to_string();
    let total_staging = make_staging(&total_id);
    for name in ["control.sqlite", "scheduler.sqlite"] {
        fs::write(total_staging.join(name), b"abc").unwrap();
        fs::set_permissions(total_staging.join(name), fs::Permissions::from_mode(0o600)).unwrap();
    }
    assert!(crate::cleanup_restore_staging(&target, &total_id, 2, 4, 5).is_err());

    let hardlink_id = uuid::Uuid::now_v7().hyphenated().to_string();
    let hardlink_staging = make_staging(&hardlink_id);
    let first = hardlink_staging.join("control.sqlite");
    fs::write(&first, b"x").unwrap();
    fs::set_permissions(&first, fs::Permissions::from_mode(0o600)).unwrap();
    fs::hard_link(&first, hardlink_staging.join("scheduler.sqlite")).unwrap();
    assert!(crate::cleanup_restore_staging(&target, &hardlink_id, 2, 1, 2).is_err());

    let receipt_id = uuid::Uuid::now_v7().hyphenated().to_string();
    make_staging(&receipt_id);
    fs::create_dir(parent.join(format!(".cleanup-target.restore-failure-{receipt_id}.json")))
        .unwrap();
    assert!(crate::cleanup_restore_staging(&target, &receipt_id, 1, 1, 1).is_err());

    let held = crate::RestoreTarget::acquire(&target).unwrap();
    let held_name = held.staging_root().file_name().unwrap().to_str().unwrap();
    let held_id = held_name.rsplit_once(".restore-").unwrap().1;
    assert_eq!(
        crate::cleanup_restore_staging(&target, held_id, 1, 1, 1)
            .unwrap_err()
            .code(),
        ErrorCode::DataDirInUse
    );
}
