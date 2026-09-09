use super::*;

#[test]
fn p1_admission_lock_restore_target_and_current_schema_fail_closed() {
    let (tmp, root) = unique_root();
    let mut config = storage_config(&root);
    config.free_space_hard_bytes = u64::MAX - 1;
    let hardening = HardeningConfig {
        emergency_reserve_bytes: 1,
        ..HardeningConfig::default()
    };
    let storage =
        PlatformStorage::bootstrap_with_hardening(&config, &hardening, &SystemClock).unwrap();
    assert_eq!(
        storage.reserve_mutation(1).unwrap_err().code(),
        ErrorCode::StoragePressure
    );
    assert_eq!(
        DataDir::acquire_existing_offline(&config)
            .unwrap_err()
            .code(),
        ErrorCode::DataDirInUse
    );
    let scheduler_path = storage.data_dir().ensure_scheduler_db().unwrap();
    drop(crate::SchedulerStore::open(&scheduler_path, 5_000, 1).unwrap());
    drop(storage);

    let data_dir = DataDir::acquire_existing_offline(&config).unwrap();
    let control =
        crate::ControlDb::open_readonly_wal_aware(&data_dir.control_db_path(), 5_000).unwrap();
    let current = crate::inspect_current_schema(&data_dir, &control, 5_000).unwrap();
    assert_eq!(
        i64::from(current.control),
        crate::migrations::current_schema_version()
    );
    assert_eq!(
        crate::inspect_current_schema(&data_dir, &control, 5_000).unwrap(),
        current
    );
    drop(control);
    drop(data_dir);

    let target = fs::canonicalize(tmp.path()).unwrap().join("restored");
    fs::create_dir(&target).unwrap();
    fs::write(target.join("occupied"), b"x").unwrap();
    assert_eq!(
        crate::RestoreTarget::acquire(&target).unwrap_err().code(),
        ErrorCode::RestoreInvalid
    );
    fs::remove_file(target.join("occupied")).unwrap();
    let restore = crate::RestoreTarget::acquire(&target).unwrap();
    assert!(restore.staging_root().starts_with(target.parent().unwrap()));
    assert!(restore.destination_for("../escape").is_err());
    assert_eq!(
        crate::RestoreTarget::acquire(&target).unwrap_err().code(),
        ErrorCode::DataDirInUse
    );
    let nested = restore.destination_for("do/workerd/failure.bin").unwrap();
    fs::write(&nested, b"retained restore bytes").unwrap();
    fs::set_permissions(&nested, fs::Permissions::from_mode(0o600)).unwrap();
    assert!(restore.destination_for("do/workerd/failure.bin").is_err());
    let staging_name = restore
        .staging_root()
        .file_name()
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    let staging_id = staging_name.rsplit_once(".restore-").unwrap().1.to_owned();
    drop(restore);
    let cleaned =
        crate::cleanup_restore_staging(&target, &staging_id, 10, 1024, 10 * 1024).unwrap();
    assert_eq!(cleaned.files, 1);
    assert!(!target.parent().unwrap().join(staging_name).exists());

    let real_parent = fs::canonicalize(tmp.path()).unwrap().join("restore-parent");
    fs::create_dir(&real_parent).unwrap();
    let alias_parent = fs::canonicalize(tmp.path())
        .unwrap()
        .join("restore-parent-alias");
    std::os::unix::fs::symlink(&real_parent, &alias_parent).unwrap();
    assert_eq!(
        crate::RestoreTarget::acquire(&alias_parent.join("target"))
            .unwrap_err()
            .code(),
        ErrorCode::RestoreInvalid
    );
}
