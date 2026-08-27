//! Fixed low-cardinality P2.2 Queue producer and retention metrics.

use super::{Inner, write_help};
use open_compute_storage::{CronSlotSummary, QueueCompletionSummary, QueueDlqForwardSummary};
use std::fmt::Write as _;
use std::sync::Arc;
use std::time::Duration;

#[derive(Debug, Default)]
pub(super) struct QueueMetrics {
    requests: [u64; 6],
    duration: [f64; 3],
    messages: [u64; 6],
    body_bytes: [u64; 6],
    backlog_messages: u64,
    backlog_bytes: u64,
    retention_deleted: [u64; 2],
    retention_deleted_bytes: [u64; 2],
    reconcile: [u64; 6],
    projection_lag_seconds: f64,
    result_unknown: [u64; 2],
    consumer_batches: [u64; 4],
    consumer_messages: [u64; 5],
    consumer_in_flight: u64,
    consumer_claim_latency_seconds: f64,
    consumer_handler_seconds: f64,
    consumer_stale_completions: u64,
    dlq_moves: [u64; 3],
    dlq_pending: u64,
    cron_slots: [u64; 2],
    cron_runs: [u64; 4],
    cron_in_flight: u64,
    cron_lag_seconds: f64,
    cron_stale_completions: u64,
}

/// Fixed native Queue handler result class.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum QueueConsumerBatchOutcome {
    /// Handler returned successfully.
    Success,
    /// Handler threw a known exception.
    Exception,
    /// Transport outcome is unknown and the lease remains held.
    Unknown,
    /// Native disposition failed host validation.
    Invalid,
}

/// Fixed native Cron handler/completion result class.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CronRunOutcome {
    /// Handler completed successfully.
    Success,
    /// Handler failed terminally.
    Exception,
    /// Handler failed and the logical run was rescheduled.
    Retry,
    /// Transport outcome is unknown and the lease remains held.
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum P23InFlightKind {
    Queue,
    Cron,
}

/// RAII gauge for every native Queue/Cron handler dispatch path.
#[derive(Debug)]
pub(crate) struct P23InFlightGuard {
    metrics: Arc<super::MetricsRegistry>,
    kind: P23InFlightKind,
}

impl Drop for P23InFlightGuard {
    fn drop(&mut self) {
        self.metrics.adjust_p23_in_flight(self.kind, false);
    }
}

/// Queue producer operation label.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum QueueMetricOperation {
    /// One-message send.
    Send,
    /// Atomic batch send.
    Batch,
    /// Backlog metrics query.
    Metrics,
}

impl QueueMetricOperation {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Send => "send",
            Self::Batch => "batch",
            Self::Metrics => "metrics",
        }
    }
}

/// Queue cross-database lifecycle convergence operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum QueueReconcileOperation {
    /// Create projection convergence.
    Create,
    /// Config projection convergence.
    Config,
    /// Delete projection convergence.
    Delete,
}

impl QueueReconcileOperation {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Create => "create",
            Self::Config => "config",
            Self::Delete => "delete",
        }
    }
}

pub(super) fn write_queue_metrics(out: &mut String, metrics: &Inner) {
    write_help(
        out,
        "queue_producer_requests_total",
        "counter",
        "Queue producer request outcomes",
    );
    for operation in operations() {
        for success in [false, true] {
            writeln!(
                out,
                "queue_producer_requests_total{{operation=\"{}\",outcome=\"{}\"}} {}",
                operation.as_str(),
                outcome(success),
                metrics.queue.requests[operation_index(operation) * 2 + usize::from(success)]
            )
            .ok();
        }
    }
    write_help(
        out,
        "queue_producer_duration_seconds",
        "gauge",
        "Last Queue producer request duration",
    );
    for operation in operations() {
        writeln!(
            out,
            "queue_producer_duration_seconds{{operation=\"{}\"}} {}",
            operation.as_str(),
            metrics.queue.duration[operation_index(operation)]
        )
        .ok();
    }
    write_outcome_totals(
        out,
        "queue_producer_messages_total",
        "Queue producer serialized message totals",
        &metrics.queue.messages,
    );
    write_outcome_totals(
        out,
        "queue_producer_body_bytes_total",
        "Queue producer serialized body byte totals",
        &metrics.queue.body_bytes,
    );
    write_help(
        out,
        "queue_backlog_messages",
        "gauge",
        "Aggregate durable Queue backlog messages",
    );
    writeln!(
        out,
        "queue_backlog_messages {}",
        metrics.queue.backlog_messages
    )
    .ok();
    write_help(
        out,
        "queue_backlog_bytes",
        "gauge",
        "Aggregate durable Queue backlog body bytes",
    );
    writeln!(out, "queue_backlog_bytes {}", metrics.queue.backlog_bytes).ok();
    write_retention_totals(
        out,
        "queue_retention_deleted_total",
        "Queue retention deleted message totals",
        &metrics.queue.retention_deleted,
    );
    write_retention_totals(
        out,
        "queue_retention_deleted_bytes_total",
        "Queue retention deleted body byte totals",
        &metrics.queue.retention_deleted_bytes,
    );
    write_help(
        out,
        "queue_reconcile_total",
        "counter",
        "Queue cross-database reconciliation outcomes",
    );
    for operation in reconcile_operations() {
        for success in [false, true] {
            writeln!(
                out,
                "queue_reconcile_total{{operation=\"{}\",outcome=\"{}\"}} {}",
                operation.as_str(),
                outcome(success),
                metrics.queue.reconcile[reconcile_index(operation) * 2 + usize::from(success)]
            )
            .ok();
        }
    }
    write_help(
        out,
        "queue_projection_lag_seconds",
        "gauge",
        "Oldest observed Queue projection convergence lag",
    );
    writeln!(
        out,
        "queue_projection_lag_seconds {}",
        metrics.queue.projection_lag_seconds
    )
    .ok();
    write_help(
        out,
        "queue_result_unknown_total",
        "counter",
        "Queue producer responses lost after a possibly committed mutation",
    );
    for operation in [QueueMetricOperation::Send, QueueMetricOperation::Batch] {
        writeln!(
            out,
            "queue_result_unknown_total{{operation=\"{}\"}} {}",
            operation.as_str(),
            metrics.queue.result_unknown[operation_index(operation)]
        )
        .ok();
    }
    write_p23_metrics(out, metrics);
}

fn write_p23_metrics(out: &mut String, metrics: &Inner) {
    write_labeled_values(
        out,
        "open_compute_queue_consumer_batches_total",
        "Native Queue consumer batch outcomes",
        "outcome",
        &["success", "exception", "unknown", "invalid"],
        &metrics.queue.consumer_batches,
        "counter",
    );
    write_labeled_values(
        out,
        "open_compute_queue_consumer_messages_total",
        "Durable Queue consumer message completion outcomes",
        "outcome",
        &["ack", "retry", "dead_letter", "dlq_pending", "discard"],
        &metrics.queue.consumer_messages,
        "counter",
    );
    for (name, help, value) in [
        (
            "open_compute_queue_consumer_in_flight",
            "Native Queue consumer batches currently in flight",
            metrics.queue.consumer_in_flight as f64,
        ),
        (
            "open_compute_queue_consumer_claim_latency_seconds",
            "Last durable Queue consumer claim latency",
            metrics.queue.consumer_claim_latency_seconds,
        ),
        (
            "open_compute_queue_consumer_handler_seconds",
            "Last native Queue consumer handler duration",
            metrics.queue.consumer_handler_seconds,
        ),
    ] {
        write_help(out, name, "gauge", help);
        writeln!(out, "{name} {value}").ok();
    }
    write_help(
        out,
        "open_compute_queue_consumer_stale_completions_total",
        "counter",
        "Token-fenced stale Queue consumer completions",
    );
    writeln!(
        out,
        "open_compute_queue_consumer_stale_completions_total {}",
        metrics.queue.consumer_stale_completions
    )
    .ok();
    write_labeled_values(
        out,
        "open_compute_queue_dlq_moves_total",
        "Bounded Queue DLQ forwarding outcomes",
        "outcome",
        &["moved", "deferred", "expired"],
        &metrics.queue.dlq_moves,
        "counter",
    );
    write_help(
        out,
        "open_compute_queue_dlq_pending",
        "gauge",
        "Terminal Queue messages waiting for DLQ capacity",
    );
    writeln!(
        out,
        "open_compute_queue_dlq_pending {}",
        metrics.queue.dlq_pending
    )
    .ok();
    write_labeled_values(
        out,
        "open_compute_cron_slots_total",
        "Durable Cron logical slot projection outcomes",
        "outcome",
        &["projected", "misfire_skipped"],
        &metrics.queue.cron_slots,
        "counter",
    );
    write_labeled_values(
        out,
        "open_compute_cron_runs_total",
        "Native Cron run outcomes",
        "outcome",
        &["success", "exception", "retry", "unknown"],
        &metrics.queue.cron_runs,
        "counter",
    );
    for (name, help, value) in [
        (
            "open_compute_cron_in_flight",
            "Native Cron runs currently in flight",
            metrics.queue.cron_in_flight as f64,
        ),
        (
            "open_compute_cron_lag_seconds",
            "Oldest ready Cron logical slot lag",
            metrics.queue.cron_lag_seconds,
        ),
    ] {
        write_help(out, name, "gauge", help);
        writeln!(out, "{name} {value}").ok();
    }
    write_help(
        out,
        "open_compute_cron_stale_completions_total",
        "counter",
        "Token-fenced stale Cron completions",
    );
    writeln!(
        out,
        "open_compute_cron_stale_completions_total {}",
        metrics.queue.cron_stale_completions
    )
    .ok();
}

impl super::MetricsRegistry {
    pub(crate) fn observe_queue_producer(
        &self,
        operation: QueueMetricOperation,
        success: bool,
        messages: u64,
        body_bytes: u64,
        duration: Duration,
    ) {
        let index = operation_index(operation);
        let outcome_index = index * 2 + usize::from(success);
        let mut guard = self.lock();
        guard.queue.requests[outcome_index] = guard.queue.requests[outcome_index].saturating_add(1);
        guard.queue.duration[index] = duration.as_secs_f64();
        guard.queue.messages[outcome_index] =
            guard.queue.messages[outcome_index].saturating_add(messages);
        guard.queue.body_bytes[outcome_index] =
            guard.queue.body_bytes[outcome_index].saturating_add(body_bytes);
    }

    pub(crate) fn set_queue_backlog(&self, messages: u64, bytes: u64) {
        let mut guard = self.lock();
        guard.queue.backlog_messages = messages;
        guard.queue.backlog_bytes = bytes;
    }

    pub(crate) fn observe_queue_retention(&self, success: bool, messages: u64, bytes: u64) {
        let index = usize::from(success);
        let mut guard = self.lock();
        guard.queue.retention_deleted[index] =
            guard.queue.retention_deleted[index].saturating_add(messages);
        guard.queue.retention_deleted_bytes[index] =
            guard.queue.retention_deleted_bytes[index].saturating_add(bytes);
    }

    pub(crate) fn observe_queue_reconcile(
        &self,
        operation: QueueReconcileOperation,
        success: bool,
        lag: Duration,
    ) {
        let index = reconcile_index(operation) * 2 + usize::from(success);
        let mut guard = self.lock();
        guard.queue.reconcile[index] = guard.queue.reconcile[index].saturating_add(1);
        guard.queue.projection_lag_seconds = lag.as_secs_f64();
    }

    pub(crate) fn inc_queue_result_unknown(&self, operation: QueueMetricOperation) {
        if operation == QueueMetricOperation::Metrics {
            return;
        }
        let index = operation_index(operation);
        let mut guard = self.lock();
        guard.queue.result_unknown[index] = guard.queue.result_unknown[index].saturating_add(1);
    }

    pub(crate) fn observe_queue_consumer_claim(&self, duration: Duration) {
        self.lock().queue.consumer_claim_latency_seconds = duration.as_secs_f64();
    }

    pub(crate) fn observe_queue_consumer_batch(
        &self,
        outcome: QueueConsumerBatchOutcome,
        duration: Duration,
    ) {
        let mut guard = self.lock();
        let index = queue_consumer_batch_index(outcome);
        guard.queue.consumer_batches[index] = guard.queue.consumer_batches[index].saturating_add(1);
        guard.queue.consumer_handler_seconds = duration.as_secs_f64();
    }

    pub(crate) fn observe_queue_consumer_completion(&self, summary: QueueCompletionSummary) {
        let mut guard = self.lock();
        for (index, value) in [
            summary.acknowledged,
            summary.retried,
            summary.dead_lettered,
            summary.dlq_pending,
            summary.discarded,
        ]
        .into_iter()
        .enumerate()
        {
            guard.queue.consumer_messages[index] =
                guard.queue.consumer_messages[index].saturating_add(value);
        }
    }

    pub(crate) fn inc_queue_consumer_stale_completion(&self) {
        let mut guard = self.lock();
        guard.queue.consumer_stale_completions =
            guard.queue.consumer_stale_completions.saturating_add(1);
    }

    pub(crate) fn observe_queue_dlq_forward(&self, summary: QueueDlqForwardSummary, pending: u64) {
        let mut guard = self.lock();
        for (index, value) in [summary.moved, summary.deferred, summary.expired]
            .into_iter()
            .enumerate()
        {
            guard.queue.dlq_moves[index] = guard.queue.dlq_moves[index].saturating_add(value);
        }
        guard.queue.dlq_pending = pending;
    }

    pub(crate) fn observe_cron_slots(&self, summary: CronSlotSummary) {
        let mut guard = self.lock();
        guard.queue.cron_slots[0] = guard.queue.cron_slots[0].saturating_add(summary.projected);
        guard.queue.cron_slots[1] = guard.queue.cron_slots[1].saturating_add(summary.skipped);
    }

    pub(crate) fn inc_cron_run(&self, outcome: CronRunOutcome) {
        let mut guard = self.lock();
        let index = cron_run_index(outcome);
        guard.queue.cron_runs[index] = guard.queue.cron_runs[index].saturating_add(1);
    }

    pub(crate) fn set_cron_lag(&self, lag_seconds: f64) {
        self.lock().queue.cron_lag_seconds = lag_seconds.max(0.0);
    }

    pub(crate) fn inc_cron_stale_completion(&self) {
        let mut guard = self.lock();
        guard.queue.cron_stale_completions = guard.queue.cron_stale_completions.saturating_add(1);
    }

    pub(crate) fn track_queue_consumer(self: &Arc<Self>) -> P23InFlightGuard {
        self.adjust_p23_in_flight(P23InFlightKind::Queue, true);
        P23InFlightGuard {
            metrics: self.clone(),
            kind: P23InFlightKind::Queue,
        }
    }

    pub(crate) fn track_cron(self: &Arc<Self>) -> P23InFlightGuard {
        self.adjust_p23_in_flight(P23InFlightKind::Cron, true);
        P23InFlightGuard {
            metrics: self.clone(),
            kind: P23InFlightKind::Cron,
        }
    }

    fn adjust_p23_in_flight(&self, kind: P23InFlightKind, starting: bool) {
        let mut guard = self.lock();
        let value = match kind {
            P23InFlightKind::Queue => &mut guard.queue.consumer_in_flight,
            P23InFlightKind::Cron => &mut guard.queue.cron_in_flight,
        };
        *value = if starting {
            value.saturating_add(1)
        } else {
            value.saturating_sub(1)
        };
    }
}

fn write_labeled_values(
    out: &mut String,
    name: &str,
    help: &str,
    label: &str,
    labels: &[&str],
    values: &[u64],
    metric_type: &str,
) {
    write_help(out, name, metric_type, help);
    for (value, label_value) in values.iter().zip(labels) {
        writeln!(out, "{name}{{{label}=\"{label_value}\"}} {value}").ok();
    }
}

const fn queue_consumer_batch_index(outcome: QueueConsumerBatchOutcome) -> usize {
    match outcome {
        QueueConsumerBatchOutcome::Success => 0,
        QueueConsumerBatchOutcome::Exception => 1,
        QueueConsumerBatchOutcome::Unknown => 2,
        QueueConsumerBatchOutcome::Invalid => 3,
    }
}

const fn cron_run_index(outcome: CronRunOutcome) -> usize {
    match outcome {
        CronRunOutcome::Success => 0,
        CronRunOutcome::Exception => 1,
        CronRunOutcome::Retry => 2,
        CronRunOutcome::Unknown => 3,
    }
}

fn write_outcome_totals(out: &mut String, name: &str, help: &str, values: &[u64; 6]) {
    write_help(out, name, "counter", help);
    for operation in operations() {
        for success in [false, true] {
            writeln!(
                out,
                "{name}{{operation=\"{}\",outcome=\"{}\"}} {}",
                operation.as_str(),
                outcome(success),
                values[operation_index(operation) * 2 + usize::from(success)]
            )
            .ok();
        }
    }
}

fn write_retention_totals(out: &mut String, name: &str, help: &str, values: &[u64; 2]) {
    write_help(out, name, "counter", help);
    for success in [false, true] {
        writeln!(
            out,
            "{name}{{outcome=\"{}\"}} {}",
            outcome(success),
            values[usize::from(success)]
        )
        .ok();
    }
}

const fn operation_index(operation: QueueMetricOperation) -> usize {
    match operation {
        QueueMetricOperation::Send => 0,
        QueueMetricOperation::Batch => 1,
        QueueMetricOperation::Metrics => 2,
    }
}

const fn reconcile_index(operation: QueueReconcileOperation) -> usize {
    match operation {
        QueueReconcileOperation::Create => 0,
        QueueReconcileOperation::Config => 1,
        QueueReconcileOperation::Delete => 2,
    }
}

const fn outcome(success: bool) -> &'static str {
    if success { "success" } else { "error" }
}

const fn operations() -> [QueueMetricOperation; 3] {
    [
        QueueMetricOperation::Send,
        QueueMetricOperation::Batch,
        QueueMetricOperation::Metrics,
    ]
}

const fn reconcile_operations() -> [QueueReconcileOperation; 3] {
    [
        QueueReconcileOperation::Create,
        QueueReconcileOperation::Config,
        QueueReconcileOperation::Delete,
    ]
}
