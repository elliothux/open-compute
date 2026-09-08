use super::*;

#[test]
fn metrics_workerd_version_and_preflight_counters() {
    let reg = MetricsRegistry::new(&MetricsConfig::default(), "0.1.0", "unknown").unwrap();
    reg.set_workerd_version("workerd 2026-08-26").unwrap();
    let text = reg.render(&PlatformStatus::starting());
    assert!(text.contains("workerd_version=\"workerd 2026-08-26\""));
    assert!(!text.contains("workerd_version=\"unknown\""));

    let outcome = open_compute_artifacts::PreflightOutcome::successful_canary();
    reg.observe_preflight_success(&outcome);
    assert_eq!(reg.object_total(ObjectOp::Put, ObjectResult::Success), 1);
    assert_eq!(reg.object_total(ObjectOp::Head, ObjectResult::Success), 2);
    assert_eq!(reg.object_total(ObjectOp::Get, ObjectResult::Success), 1);
    assert_eq!(reg.object_total(ObjectOp::Delete, ObjectResult::Success), 1);
    assert_eq!(reg.object_total(ObjectOp::Put, ObjectResult::Failure), 0);
}
