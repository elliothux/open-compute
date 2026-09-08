use super::*;

#[tokio::test]
async fn doctor_skips_db_when_platform_lock_is_held() {
    let (dir, path, mock) = initialized_doctor_fixture().await;
    let loaded = load_fixture_platform_config(&path);
    let _storage = open_compute_storage::PlatformStorage::bootstrap(
        &loaded.config.data,
        &open_compute_core::SystemClock,
    )
    .expect("hold lock");
    let before = content_snapshot(&loaded.config.data.path);
    let report = doctor_report(&loaded, DoctorMode::Full).await;
    assert_eq!(check(&report, "lock").status, CheckStatus::Failed);
    assert_eq!(check(&report, "lock").code, Some("DATA_DIR_IN_USE"));
    assert_eq!(check(&report, "sqlite").status, CheckStatus::Skipped);
    assert_eq!(check(&report, "schema").status, CheckStatus::Skipped);
    assert_eq!(check(&report, "identity").status, CheckStatus::Skipped);
    assert_eq!(
        check(&report, "cache_integrity").status,
        CheckStatus::Skipped
    );
    assert_eq!(
        check(&report, "object_storage_canary").status,
        CheckStatus::Skipped
    );
    assert_eq!(check(&report, "runtime_cycle").status, CheckStatus::Skipped);
    assert_eq!(content_snapshot(&loaded.config.data.path), before);
    assert_eq!(mock.object_count(), 1);
    let _ = dir;
}
