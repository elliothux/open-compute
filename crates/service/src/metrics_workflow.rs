//! Compile-time-only Workflow series; no tenant identity or exception labels.

use super::{Inner, MetricsRegistry, write_help};
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
    statuses: [u64; 4],
    in_flight: u64,
    lag_seconds: f64,
    state_bytes: u64,
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
    outcome: WorkflowOutcome,
}
impl WorkflowRunGuard {
    pub(crate) fn finish(&mut self, outcome: WorkflowOutcome) {
        self.outcome = outcome;
    }
}
impl Drop for WorkflowRunGuard {
    fn drop(&mut self) {
        if let Ok(mut inner) = self.metrics.inner.lock() {
            let metrics = &mut inner.workflow;
            metrics.in_flight = metrics.in_flight.saturating_sub(1);
            metrics.runs[self.outcome.index()] =
                metrics.runs[self.outcome.index()].saturating_add(1);
            metrics.run_seconds[self.outcome.index()] = self.started.elapsed().as_secs_f64();
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
            outcome: WorkflowOutcome::Unknown,
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
            inner.workflow.statuses = [
                summary.queued,
                summary.running,
                summary.complete,
                summary.errored,
            ];
            inner.workflow.state_bytes = summary.state_bytes;
            inner.workflow.lag_seconds = lag_seconds;
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
    for (index, status) in ["queued", "running", "complete", "errored"]
        .iter()
        .enumerate()
    {
        writeln!(
            out,
            "open_compute_workflow_instance_status{{status=\"{status}\"}} {}",
            metrics.statuses[index]
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
            metrics.state_bytes as f64,
        ),
    ] {
        write_help(out, name, "gauge", "Workflow bounded workload gauge");
        writeln!(out, "{name} {value}").ok();
    }
}
