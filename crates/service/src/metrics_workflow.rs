//! Compile-time-only Workflow series; no tenant identity or exception labels.

use super::{Inner, MetricsRegistry, write_help};
use open_compute_core::ErrorCode;
use open_compute_storage::WorkflowOperationInspection;
use open_compute_storage::scheduler::WorkflowInspection;
use std::fmt::Write as _;
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Debug, Default)]
pub(super) struct WorkflowMetrics {
    instances: [u64; 3],
    runs: [u64; 3],
    run_seconds: [f64; 3],
    steps: [u64; 3],
    step_seconds: [f64; 3],
    replay: [u64; 2],
    stale: [u64; 2],
    reconcile: [u64; 2],
    summary: WorkflowInspection,
    operations: WorkflowOperationInspection,
    operation_age_seconds: f64,
    event_intake: [u64; 3],
    lifecycle: [[u64; 2]; 4],
    in_flight: u64,
    lag_seconds: f64,
    suspended_runs: u64,
    suspended_seconds: f64,
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum WorkflowOutcome {
    Success,
    Error,
    Unknown,
}
impl WorkflowOutcome {
    const fn index(self) -> usize {
        match self {
            Self::Success => 0,
            Self::Error => 1,
            Self::Unknown => 2,
        }
    }
}

#[derive(Debug)]
pub(crate) struct WorkflowRunGuard {
    metrics: Arc<MetricsRegistry>,
    started: Instant,
    outcome: Option<WorkflowOutcome>,
}
impl WorkflowRunGuard {
    pub(crate) fn finish(&mut self, outcome: WorkflowOutcome) {
        self.outcome = Some(outcome);
    }
    pub(crate) fn suspended(&mut self) {
        self.outcome = None;
    }
}
impl Drop for WorkflowRunGuard {
    fn drop(&mut self) {
        if let Ok(mut inner) = self.metrics.inner.lock() {
            let metrics = &mut inner.workflow;
            metrics.in_flight = metrics.in_flight.saturating_sub(1);
            if let Some(outcome) = self.outcome {
                metrics.runs[outcome.index()] = metrics.runs[outcome.index()].saturating_add(1);
                metrics.run_seconds[outcome.index()] = self.started.elapsed().as_secs_f64();
            } else {
                metrics.suspended_runs = metrics.suspended_runs.saturating_add(1);
                metrics.suspended_seconds = self.started.elapsed().as_secs_f64();
            }
        }
    }
}

impl MetricsRegistry {
    pub(crate) fn workflow_run(self: &Arc<Self>) -> WorkflowRunGuard {
        if let Ok(mut inner) = self.inner.lock() {
            inner.workflow.in_flight = inner.workflow.in_flight.saturating_add(1);
        }
        WorkflowRunGuard {
            metrics: self.clone(),
            started: Instant::now(),
            outcome: Some(WorkflowOutcome::Unknown),
        }
    }
    pub(crate) fn workflow_created(&self, outcome: WorkflowOutcome) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.workflow.instances[outcome.index()] =
                inner.workflow.instances[outcome.index()].saturating_add(1);
        }
    }
    pub(crate) fn workflow_step(&self, outcome: WorkflowOutcome, duration: Duration) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.workflow.steps[outcome.index()] =
                inner.workflow.steps[outcome.index()].saturating_add(1);
            inner.workflow.step_seconds[outcome.index()] = duration.as_secs_f64();
        }
    }
    pub(crate) fn workflow_replay(&self, failed: bool) {
        if let Ok(mut inner) = self.inner.lock() {
            let count = &mut inner.workflow.replay[usize::from(failed)];
            *count = count.saturating_add(1);
        }
    }
    pub(crate) fn workflow_stale(&self, step: bool) {
        if let Ok(mut inner) = self.inner.lock() {
            let count = &mut inner.workflow.stale[usize::from(step)];
            *count = count.saturating_add(1);
        }
    }
    pub(crate) fn workflow_reconcile(&self, success: bool) {
        if let Ok(mut inner) = self.inner.lock() {
            let count = &mut inner.workflow.reconcile[usize::from(!success)];
            *count = count.saturating_add(1);
        }
    }
    pub(crate) fn workflow_summary(&self, summary: &WorkflowInspection, lag_seconds: f64) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.workflow.summary = summary.clone();
            inner.workflow.lag_seconds = lag_seconds;
        }
    }
    pub(crate) fn workflow_operations(&self, summary: &WorkflowOperationInspection, now_ms: i64) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.workflow.operations = summary.clone();
            inner.workflow.operation_age_seconds = summary
                .oldest_operation_at_ms
                .map_or(0.0, |at| now_ms.saturating_sub(at).max(0) as f64 / 1000.0);
        }
    }
    pub(crate) fn workflow_event(&self, failure: Option<ErrorCode>) {
        let index = match failure {
            None => 0,
            Some(ErrorCode::WorkflowEventQueueFull) => 1,
            Some(_) => 2,
        };
        if let Ok(mut inner) = self.inner.lock() {
            let count = &mut inner.workflow.event_intake[index];
            *count = count.saturating_add(1);
        }
    }
    pub(crate) fn workflow_lifecycle(&self, operation: &str, success: bool) {
        let index = match operation {
            "pause" => 0,
            "resume" => 1,
            "terminate" => 2,
            "restart" => 3,
            _ => return,
        };
        if let Ok(mut inner) = self.inner.lock() {
            let count = &mut inner.workflow.lifecycle[index][usize::from(!success)];
            *count = count.saturating_add(1);
        }
    }
}

pub(super) fn write_workflow_metrics(out: &mut String, inner: &Inner) {
    let metrics = &inner.workflow;
    for (name, counts) in [
        ("open_compute_workflow_instances_total", &metrics.instances),
        ("open_compute_workflow_runs_total", &metrics.runs),
        ("open_compute_workflow_steps_total", &metrics.steps),
    ] {
        write_help(out, name, "counter", "Workflow durable operation outcomes");
        for (index, label) in ["success", "error", "unknown"].iter().enumerate() {
            writeln!(out, "{name}{{outcome=\"{label}\"}} {}", counts[index]).ok();
        }
    }
    for (name, values) in [
        ("open_compute_workflow_run_seconds", &metrics.run_seconds),
        ("open_compute_workflow_step_seconds", &metrics.step_seconds),
    ] {
        write_help(
            out,
            name,
            "gauge",
            "Last Workflow operation duration in seconds",
        );
        for (index, label) in ["success", "error", "unknown"].iter().enumerate() {
            writeln!(out, "{name}{{outcome=\"{label}\"}} {}", values[index]).ok();
        }
    }
    writeln!(
        out,
        "open_compute_workflow_runs_total{{outcome=\"suspended\"}} {}",
        metrics.suspended_runs
    )
    .ok();
    writeln!(
        out,
        "open_compute_workflow_run_seconds{{outcome=\"suspended\"}} {}",
        metrics.suspended_seconds
    )
    .ok();
    for (name, label, labels, counts) in [
        (
            "open_compute_workflow_replay_steps_total",
            "outcome",
            ["complete", "failed"],
            &metrics.replay,
        ),
        (
            "open_compute_workflow_stale_commits_total",
            "kind",
            ["run", "step"],
            &metrics.stale,
        ),
        (
            "open_compute_workflow_reconcile_total",
            "outcome",
            ["success", "error"],
            &metrics.reconcile,
        ),
    ] {
        write_help(
            out,
            name,
            "counter",
            "Workflow replay and integrity outcomes",
        );
        for (index, value) in labels.iter().enumerate() {
            writeln!(out, "{name}{{{label}=\"{value}\"}} {}", counts[index]).ok();
        }
    }
    write_help(
        out,
        "open_compute_workflow_instance_status",
        "gauge",
        "Retained Workflow instances by state",
    );
    let summary = &metrics.summary;
    for (status, count) in [
        ("queued", summary.queued),
        ("running", summary.running),
        ("waiting", summary.waiting),
        ("paused", summary.paused),
        ("complete", summary.complete),
        ("errored", summary.errored),
        ("terminated", summary.terminated),
    ] {
        writeln!(
            out,
            "open_compute_workflow_instance_status{{status=\"{status}\"}} {count}"
        )
        .ok();
    }
    for (name, value) in [
        ("open_compute_workflow_in_flight", metrics.in_flight as f64),
        (
            "open_compute_workflow_queue_lag_seconds",
            metrics.lag_seconds,
        ),
        (
            "open_compute_workflow_state_bytes",
            summary.state_bytes as f64,
        ),
        (
            "open_compute_workflow_retained_instances",
            summary.retained as f64,
        ),
        (
            "open_compute_workflow_inbox_bytes",
            summary.inbox_bytes as f64,
        ),
        (
            "open_compute_workflow_buffered_events",
            summary.buffered_events as f64,
        ),
        (
            "open_compute_workflow_consumed_events",
            summary.consumed_events as f64,
        ),
        (
            "open_compute_workflow_operation_age_seconds",
            metrics.operation_age_seconds,
        ),
    ] {
        write_help(out, name, "gauge", "Workflow bounded workload gauge");
        writeln!(out, "{name} {value}").ok();
    }
    for (name, label, values) in [
        (
            "open_compute_workflow_waiting_steps",
            "reason",
            [
                ("sleep", summary.sleeping_steps),
                ("event", summary.event_waits),
                ("retry", summary.retry_waits),
            ]
            .as_slice(),
        ),
        (
            "open_compute_workflow_retry_results",
            "outcome",
            [
                ("complete", summary.retried_steps),
                ("exhausted", summary.exhausted_steps),
            ]
            .as_slice(),
        ),
        (
            "open_compute_workflow_timeout_results",
            "kind",
            [
                ("step", summary.step_timeouts),
                ("event", summary.event_timeouts),
            ]
            .as_slice(),
        ),
        (
            "open_compute_workflow_pending_operations",
            "phase",
            [
                ("restart_intent", metrics.operations.pending_restarts),
                ("purge_intent", metrics.operations.pending_purges),
                ("purge_receipt", summary.gc_receipts),
            ]
            .as_slice(),
        ),
    ] {
        write_help(
            out,
            name,
            "gauge",
            "Workflow retained authority facts; restart and purge may reduce these gauges",
        );
        for (value, count) in values {
            writeln!(out, "{name}{{{label}=\"{value}\"}} {count}").ok();
        }
    }
    write_help(
        out,
        "open_compute_workflow_event_intake_total",
        "counter",
        "Workflow event intake outcomes observed by this process",
    );
    for (index, outcome) in ["accepted", "full", "error"].iter().enumerate() {
        writeln!(
            out,
            "open_compute_workflow_event_intake_total{{outcome=\"{outcome}\"}} {}",
            metrics.event_intake[index]
        )
        .ok();
    }
    write_help(
        out,
        "open_compute_workflow_lifecycle_total",
        "counter",
        "Workflow lifecycle outcomes observed by this process",
    );
    for (index, operation) in ["pause", "resume", "terminate", "restart"]
        .iter()
        .enumerate()
    {
        for (result, outcome) in ["success", "error"].iter().enumerate() {
            writeln!(out,"open_compute_workflow_lifecycle_total{{operation=\"{operation}\",outcome=\"{outcome}\"}} {}",metrics.lifecycle[index][result]).ok();
        }
    }
}
