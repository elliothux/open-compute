use super::*;
use open_compute_core::{MetricsConfig, PlatformStatus};

#[test]
fn every_fixed_operation_and_outcome_is_observable() {
    let metrics =
        Arc::new(MetricsRegistry::new(&MetricsConfig::default(), "test", "workerd").unwrap());
    for operation in cache_operations() {
        metrics.observe_response_cache(operation, true);
        metrics.observe_response_cache(operation, false);
    }
    metrics.set_response_cache_stats(CacheStats {
        entries: 2,
        body_bytes: 3,
        metadata_bytes: 5,
        active_refreshes: 7,
        open_databases: 11,
    });
    metrics.observe_response_cache_s3(CacheS3Operation::Get, Duration::from_millis(5));
    metrics.observe_response_cache_s3(CacheS3Operation::Put, Duration::from_secs(11));

    for operation in image_operations() {
        for outcome in [
            ImageMetricOutcome::Success,
            ImageMetricOutcome::Failure,
            ImageMetricOutcome::Limit,
        ] {
            metrics.observe_image(operation, outcome);
        }
    }
    metrics.add_image_bytes(true, 13);
    metrics.add_image_bytes(false, 17);
    metrics.set_image_active_sessions(19);
    {
        let _transform = metrics.image_transform();
        assert!(
            metrics
                .render(&PlatformStatus::starting())
                .contains("images_active_transforms 1")
        );
    }

    let rendered = metrics.render(&PlatformStatus::starting());
    for expected in [
        "response_cache_operations_total{operation=\"lookup\",outcome=\"success\"} 1",
        "response_cache_operations_total{operation=\"refresh\",outcome=\"failure\"} 1",
        "response_cache_metadata_bytes 5",
        "response_cache_body_bytes 3",
        "response_cache_active_refreshes 7",
        "response_cache_open_databases 11",
        "response_cache_s3_duration_seconds_count{operation=\"get\"} 1",
        "response_cache_s3_duration_seconds_count{operation=\"put\"} 1",
        "images_operations_total{operation=\"draw\",outcome=\"limit\"} 1",
        "images_operations_total{operation=\"output\",outcome=\"failure\"} 1",
        "images_active_sessions 19",
        "images_active_transforms 0",
        "images_bytes_total{direction=\"input\"} 13",
        "images_bytes_total{direction=\"output\"} 17",
        "images_limit_rejections_total{operation=\"transform\"} 1",
    ] {
        assert!(rendered.contains(expected), "missing {expected}");
    }
}
