use super::*;

#[tokio::test]
async fn initialized_local_doctor_reports_backend_specific_checks() {
    let dir = TempDir::new().unwrap();
    let path = write_config(dir.path(), "");
    let loaded = load_fixture_platform_config(&path);
    let storage = open_compute_storage::PlatformStorage::bootstrap(
        &loaded.config.data,
        &open_compute_core::SystemClock,
    )
    .unwrap();
    let connected =
        crate::object_storage::connect_object_backend(&loaded.config, storage.identity()).unwrap();
    open_compute_artifacts::preflight_object_storage(
        &connected.backend,
        storage.identity().platform_id,
        open_compute_core::StartupId::generate(),
    )
    .await
    .unwrap();
    storage
        .bind_object_authority(
            connected.backend.kind(),
            &connected.backend.authority_sha256(),
        )
        .unwrap();
    drop(connected);
    drop(storage);

    let report = doctor_report(&loaded, DoctorMode::Basic).await;
    assert_eq!(check(&report, "local_root").status, CheckStatus::Ok);
    assert_eq!(check(&report, "local_format").status, CheckStatus::Ok);
    assert_eq!(check(&report, "local_free_space").status, CheckStatus::Ok);
    assert_eq!(check(&report, "local_fsync").status, CheckStatus::Skipped);
    assert!(
        !report
            .checks
            .iter()
            .any(|check| check.name.starts_with("s3_"))
    );
}
