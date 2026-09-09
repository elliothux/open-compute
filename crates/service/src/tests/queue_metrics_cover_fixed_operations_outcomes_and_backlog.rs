use super::*;

#[test]
fn queue_metrics_cover_fixed_operations_outcomes_and_backlog() {
    let metrics = MetricsRegistry::new(&MetricsConfig::default(), "test", "workerd").unwrap();
    for operation in [
        QueueMetricOperation::Send,
        QueueMetricOperation::Batch,
        QueueMetricOperation::Metrics,
    ] {
        metrics.observe_queue_producer(operation, false, 2, 3, Duration::from_millis(4));
        metrics.observe_queue_producer(operation, true, 5, 7, Duration::from_millis(8));
        metrics.inc_queue_result_unknown(operation);
    }
    metrics.set_queue_backlog(11, 13);
    metrics.observe_queue_retention(false, 17, 19);
    metrics.observe_queue_retention(true, 23, 29);
    for operation in [
        QueueReconcileOperation::Create,
        QueueReconcileOperation::Config,
        QueueReconcileOperation::Delete,
    ] {
        metrics.observe_queue_reconcile(operation, false, Duration::from_millis(31));
        metrics.observe_queue_reconcile(operation, true, Duration::from_millis(37));
    }
    let rendered = metrics.render(&PlatformStatus::starting());
    for expected in [
        "queue_producer_requests_total{operation=\"send\",outcome=\"error\"} 1",
        "queue_producer_requests_total{operation=\"batch\",outcome=\"success\"} 1",
        "queue_producer_messages_total{operation=\"metrics\",outcome=\"success\"} 5",
        "queue_producer_body_bytes_total{operation=\"send\",outcome=\"success\"} 7",
        "queue_backlog_messages 11",
        "queue_backlog_bytes 13",
        "queue_retention_deleted_total{outcome=\"error\"} 17",
        "queue_retention_deleted_bytes_total{outcome=\"success\"} 29",
        "queue_reconcile_total{operation=\"delete\",outcome=\"success\"} 1",
        "queue_projection_lag_seconds 0.037",
        "queue_result_unknown_total{operation=\"send\"} 1",
        "queue_result_unknown_total{operation=\"batch\"} 1",
    ] {
        assert!(rendered.contains(expected), "missing {expected}");
    }
    assert!(!rendered.contains("queue_result_unknown_total{operation=\"metrics\"}"));
}
