use super::*;

#[tokio::test]
async fn full_doctor_reports_object_storage_canary_failure_without_leaking_objects() {
    let (dir, path, mock) = initialized_doctor_fixture().await;
    let loaded = load_fixture_platform_config(&path);
    mock.set_fault(open_compute_artifacts::Fault::Permission);
    let report = doctor_report(&loaded, DoctorMode::Full).await;
    assert_eq!(
        check(&report, "object_storage_connectivity").status,
        CheckStatus::Failed
    );
    assert_eq!(
        check(&report, "object_storage_canary").status,
        CheckStatus::Failed
    );
    assert_eq!(mock.object_count(), 1);
    let _ = dir;
}
