#[test]
fn explicit_corrupt_recovery_refuses_to_move_shared_control() {
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
    let path = storage.data_dir().control_db_path();
    let before = std::fs::read(&path).unwrap();
    let error = storage
        .data_dir()
        .recover_corrupt_scheduler_db("scheduler-corrupt-test", 100, 10)
        .unwrap_err();
    assert!(error.to_string().contains("full snapshot restore"));
    assert_eq!(std::fs::read(path).unwrap(), before);
    storage.db().quick_check().unwrap();
    assert!(!root.join("scheduler.sqlite").exists());
}
