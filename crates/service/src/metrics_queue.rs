//! Fixed low-cardinality P2.2 Queue producer and retention metrics.

use super::{Inner, write_help};
use std::fmt::Write as _;
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
