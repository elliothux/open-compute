use super::*;

#[test]
fn metrics_fixed_and_limits() {
    let cfg = MetricsConfig {
        max_series: 4,
        ..MetricsConfig::default()
    };
    assert_eq!(
        MetricsRegistry::validate_limits(&cfg).unwrap_err().code(),
        ErrorCode::LimitInvalid
    );
    let cfg = MetricsConfig {
        max_series: REQUIRED_SERIES,
        max_label_value_bytes: 8,
        ..MetricsConfig::default()
    };
    assert_eq!(
        MetricsRegistry::validate_limits(&cfg).unwrap_err().code(),
        ErrorCode::LimitInvalid
    );
    let reg =
        MetricsRegistry::new(&MetricsConfig::default(), "0.1.0", "workerd 2026-08-26").unwrap();
    reg.inc_start(StartResult::Success, StartStage::Config);
    reg.observe_service_invocation(
        ServiceMetricOperation::DefaultFetch,
        true,
        Duration::from_millis(7),
    );
    reg.set_service_invocation_counts(2, 3, 5);
    let text = reg.render(&PlatformStatus::starting());
    assert!(text.contains("platform_info"));
    assert!(text.contains("platform_ready"));
    assert!(
        text.contains(
            "service_invocations_total{operation=\"default_fetch\",outcome=\"success\"} 1"
        )
    );
    assert!(
        text.contains("service_invocation_duration_seconds{operation=\"default_fetch\"} 0.007")
    );
    assert!(text.contains("service_invocation_roots 2"));
    assert!(text.contains("service_invocation_operations 3"));
    assert!(text.contains("service_capability_retentions 5"));
    assert!(text.contains("response_cache_operations_total"));
    assert!(text.contains("response_cache_object_duration_seconds_bucket"));
    assert!(text.contains("images_operations_total"));
    assert!(text.contains("images_limit_rejections_total"));
    let series = text
        .lines()
        .filter(|l| !l.starts_with('#') && !l.is_empty())
        .count();
    assert_eq!(series as u64, REQUIRED_SERIES);
    let again = reg.render(&PlatformStatus::starting());
    assert_eq!(text, again);
    assert!(text.contains("content") || text.contains("platform_info"));
    assert!(!text.contains("AKIA"));
}
