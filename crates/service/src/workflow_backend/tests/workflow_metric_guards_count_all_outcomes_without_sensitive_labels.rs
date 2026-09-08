use super::*;

#[test]
fn workflow_metric_guards_count_all_outcomes_without_sensitive_labels() {
    let f = fixture();
    for outcome in [
        WorkflowOutcome::Success,
        WorkflowOutcome::Error,
        WorkflowOutcome::Unknown,
    ] {
        let mut guard = f.metrics.workflow_run();
        guard.finish(outcome);
        f.metrics.workflow_created(outcome);
        f.metrics.workflow_step(outcome, Duration::from_millis(5));
    }
    f.metrics.workflow_reconcile(true);
    f.metrics.workflow_reconcile(false);
    f.metrics.workflow_stale(true);
    f.metrics.workflow_stale(false);
    f.metrics.workflow_summary(
        &open_compute_storage::scheduler::WorkflowInspection {
            queued: 1,
            running: 2,
            complete: 3,
            errored: 4,
            state_bytes: 100,
            expired_runs: 1,
            waiting: 5,
            paused: 6,
            terminated: 7,
            retained: 8,
            buffered_events: 2,
            inbox_bytes: 64,
            consumed_events: 3,
            sleeping_steps: 2,
            event_waits: 1,
            retry_waits: 4,
            retried_steps: 3,
            exhausted_steps: 1,
            step_timeouts: 1,
            event_timeouts: 2,
            gc_receipts: 1,
        },
        0.5,
    );
    f.metrics.workflow_operations(
        &open_compute_storage::WorkflowOperationInspection {
            pending_restarts: 1,
            pending_purges: 2,
            oldest_operation_at_ms: Some(1000),
        },
        2500,
    );
    for failure in [
        None,
        Some(ErrorCode::WorkflowEventQueueFull),
        Some(ErrorCode::WorkflowInstanceBusy),
    ] {
        f.metrics.workflow_event(failure);
    }
    for operation in ["pause", "resume", "terminate", "restart", "private-label"] {
        f.metrics.workflow_lifecycle(operation, true);
        f.metrics.workflow_lifecycle(operation, false);
    }
    let output = f
        .metrics
        .render(&crate::health::HealthCoordinator::new().snapshot());
    assert!(output.contains("open_compute_workflow_in_flight 0"));
    assert!(output.contains("open_compute_workflow_runs_total{outcome=\"unknown\"} 1"));
    assert!(output.contains("open_compute_workflow_instance_status{status=\"complete\"} 3"));
    for line in [
        "open_compute_workflow_instance_status{status=\"paused\"} 6",
        "open_compute_workflow_instance_status{status=\"running\"} 2",
        "open_compute_workflow_waiting_steps{reason=\"retry\"} 4",
        "open_compute_workflow_pending_operations{phase=\"purge_receipt\"} 1",
        "open_compute_workflow_event_intake_total{outcome=\"full\"} 1",
        "open_compute_workflow_lifecycle_total{operation=\"restart\",outcome=\"error\"} 1",
        "open_compute_workflow_operation_age_seconds 1.5",
        "open_compute_workflow_consumed_events 3",
    ] {
        assert!(output.contains(line), "missing {line}");
    }
    assert!(!output.contains("private-label"));
}
