use super::*;

#[tokio::test]
async fn doctor_reports_limits_space_and_full_prerequisite_failures() {
    let (dir, path, _mock) = initialized_doctor_fixture().await;
    let mut loaded = load_fixture_platform_config(&path);
    loaded.config.metrics.max_series = 1;
    loaded.config.data.free_space_hard_bytes = u64::MAX;
    loaded.config.data.free_space_soft_bytes = u64::MAX;
    let report = doctor_report(&loaded, DoctorMode::Basic).await;
    assert_eq!(check(&report, "config").status, CheckStatus::Failed);
    assert_eq!(check(&report, "free_space").status, CheckStatus::Failed);

    let mut loaded = load_fixture_platform_config(&path);
    loaded.config.data.free_space_hard_bytes = 0;
    loaded.config.data.free_space_soft_bytes = u64::MAX;
    let report = doctor_report(&loaded, DoctorMode::Basic).await;
    assert_eq!(check(&report, "free_space").status, CheckStatus::Warning);

    let package = open_compute_runtime::materialize_embedded_runtime(
        &loaded.config.data.path.join("runtime"),
    )
    .unwrap();
    let asset = package.assets_dir().join("config.capnp");
    fs::set_permissions(&asset, fs::Permissions::from_mode(0o600)).unwrap();
    fs::write(&asset, b"tampered").unwrap();
    let report = doctor_report(&loaded, DoctorMode::Full).await;
    assert_eq!(check(&report, "runtime_binary").status, CheckStatus::Failed);
    assert_eq!(check(&report, "runtime_cycle").status, CheckStatus::Failed);
    let _ = dir;
}
