use super::*;

#[test]
fn explicit_corrupt_recovery_quarantines_files_and_refuses_healthy_authority() {
    use std::os::unix::fs::OpenOptionsExt as _;

    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("data");
    let config = open_compute_core::DataConfig {
        path: root.clone(),
        master_key_file: temp.path().join("master.key"),
        master_key_env: None,
        sqlite_busy_timeout_ms: 100,
        free_space_soft_bytes: 2,
        free_space_hard_bytes: 1,
    };
    let storage =
        crate::PlatformStorage::bootstrap(&config, &open_compute_core::SystemClock).unwrap();
    let data_dir = storage.data_dir();
    let scheduler = data_dir.ensure_scheduler_db().unwrap();
    std::fs::write(&scheduler, b"not a sqlite database").unwrap();
    let wal = std::path::PathBuf::from(format!("{}-wal", scheduler.display()));
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&wal)
        .unwrap();

    assert!(
        data_dir
            .recover_corrupt_scheduler_db("invalid", 100, 10)
            .is_err()
    );
    let backup = data_dir
        .recover_corrupt_scheduler_db("scheduler-corrupt-test", 100, 10)
        .unwrap();
    assert_eq!(
        std::fs::read(backup.join("scheduler.sqlite")).unwrap(),
        b"not a sqlite database"
    );
    assert!(backup.join("scheduler.sqlite-wal").is_file());
    assert!(inspect_scheduler_db(&scheduler, 100, 10).is_ok());
    assert!(
        data_dir
            .recover_corrupt_scheduler_db("scheduler-corrupt-second", 100, 10)
            .is_err()
    );
}
