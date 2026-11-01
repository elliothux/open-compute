use super::*;

#[tokio::test]
async fn doctor_reports_limits_space_and_full_prerequisite_failures() {
    let (_dir, path, _mock) = initialized_doctor_fixture().await;
    let mut loaded = load_fixture_platform_config(&path);
    loaded.config.metrics.max_label_value_bytes = 1;
    loaded.config.data.free_space_hard_bytes = u64::MAX;
    loaded.config.data.free_space_soft_bytes = u64::MAX;
    let report = doctor_report(
        &loaded,
        DoctorMode::Basic,
        crate::instance_registry::ServiceScope::User,
        None,
    )
    .await;
    assert_eq!(check(&report, "config").status, CheckStatus::Failed);
    assert_eq!(check(&report, "free_space").status, CheckStatus::Failed);

    let mut loaded = load_fixture_platform_config(&path);
    loaded.config.data.free_space_hard_bytes = 0;
    loaded.config.data.free_space_soft_bytes = u64::MAX;
    let report = doctor_report(
        &loaded,
        DoctorMode::Basic,
        crate::instance_registry::ServiceScope::User,
        None,
    )
    .await;
    assert_eq!(check(&report, "free_space").status, CheckStatus::Warning);
}
