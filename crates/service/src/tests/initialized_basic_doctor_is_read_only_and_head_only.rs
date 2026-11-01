use super::*;

#[tokio::test]
async fn initialized_basic_doctor_is_read_only_and_head_only() {
    let (dir, path, mock) = initialized_doctor_fixture().await;
    let loaded = load_fixture_platform_config(&path);
    let storage = open_compute_storage::PlatformStorage::bootstrap(
        &loaded.config.data,
        &open_compute_core::SystemClock,
    )
    .unwrap();
    drop(
        SchedulerStore::open(
            &storage.data_dir().control_db_path(),
            100,
            1,
            storage.identity().instance_id,
        )
        .unwrap(),
    );
    drop(storage);
    let data = dir.path().join("data");
    let before = content_snapshot(&data);
    let wal = data.join("control.sqlite-wal");
    let shm = data.join("control.sqlite-shm");
    assert!(!wal.exists());
    let loaded = load_fixture_platform_config(&path);
    let report = doctor_report(
        &loaded,
        DoctorMode::Basic,
        crate::instance_registry::ServiceScope::User,
        None,
    )
    .await;
    assert_eq!(check(&report, "scheduler_sqlite").status, CheckStatus::Ok);
    assert_eq!(content_snapshot(&data), before);
    assert!(!wal.exists());
    assert!(!shm.exists());
    let methods: Vec<_> = mock.recorded().into_iter().map(|r| r.method).collect();
    assert!(methods.iter().all(|m| m == "HEAD"), "{methods:?}");
    assert!(!methods.is_empty());
    assert_eq!(
        check(&report, "object_storage_canary").status,
        CheckStatus::Skipped
    );
    assert_eq!(check(&report, "s3_tls").status, CheckStatus::Ok);
    assert_eq!(check(&report, "s3_connectivity").status, CheckStatus::Ok);
    assert_eq!(
        check(&report, "s3_provider_capability").status,
        CheckStatus::Skipped
    );
    let _ = dir;
}
