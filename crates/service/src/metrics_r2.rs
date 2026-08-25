//! Fixed low-cardinality P0.5 R2 metrics.

use super::{Inner, write_help};
use std::fmt::Write as _;
use std::sync::Arc;
use std::time::Duration;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum R2Operation {
    Head,
    Get,
    Put,
    Delete,
    List,
}

impl R2Operation {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Head => "head",
            Self::Get => "get",
            Self::Put => "put",
            Self::Delete => "delete",
            Self::List => "list",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum R2ProviderError {
    Availability,
    Integrity,
    ResultUnknown,
}

impl R2ProviderError {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Availability => "availability",
            Self::Integrity => "integrity",
            Self::ResultUnknown => "result_unknown",
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) enum R2StreamDirection {
    Upload,
    Download,
}

pub(crate) struct R2StreamGuard {
    metrics: Arc<super::MetricsRegistry>,
    direction: R2StreamDirection,
}

impl R2StreamGuard {
    pub(crate) fn new(metrics: &Arc<super::MetricsRegistry>, direction: R2StreamDirection) -> Self {
        metrics.adjust_r2_stream(direction, true);
        Self {
            metrics: metrics.clone(),
            direction,
        }
    }
}

impl Drop for R2StreamGuard {
    fn drop(&mut self) {
        self.metrics.adjust_r2_stream(self.direction, false);
    }
}

pub(super) fn write_r2_metrics(out: &mut String, metrics: &Inner) {
    write_help(
        out,
        "r2_operations_total",
        "counter",
        "R2 binding operation outcomes",
    );
    for operation in operations() {
        let index = operation_index(operation);
        for success in [false, true] {
            writeln!(
                out,
                "r2_operations_total{{operation=\"{}\",outcome=\"{}\"}} {}",
                operation.as_str(),
                outcome(success),
                metrics.r2_operations[index * 2 + usize::from(success)]
            )
            .ok();
        }
    }
    write_help(
        out,
        "r2_operation_duration_seconds",
        "gauge",
        "Last R2 operation duration",
    );
    for operation in operations() {
        writeln!(
            out,
            "r2_operation_duration_seconds{{operation=\"{}\",stage=\"total\"}} {}",
            operation.as_str(),
            metrics.r2_operation_duration[operation_index(operation)]
        )
        .ok();
    }
    write_help(out, "r2_bytes_total", "counter", "R2 streamed bytes");
    for (index, direction) in ["ingress", "egress"].into_iter().enumerate() {
        writeln!(
            out,
            "r2_bytes_total{{direction=\"{direction}\"}} {}",
            metrics.r2_bytes[index]
        )
        .ok();
    }
    write_help(out, "r2_active_streams", "gauge", "Active R2 streams");
    for (index, direction) in ["upload", "download"].into_iter().enumerate() {
        writeln!(
            out,
            "r2_active_streams{{direction=\"{direction}\"}} {}",
            metrics.r2_active_streams[index]
        )
        .ok();
    }
    write_help(out, "r2_staging_bytes", "gauge", "R2 staging bytes");
    writeln!(out, "r2_staging_bytes {}", metrics.r2_staging_bytes).ok();
    write_help(
        out,
        "r2_provider_errors_total",
        "counter",
        "R2 provider error categories",
    );
    for operation in operations() {
        for category in provider_errors() {
            writeln!(
                out,
                "r2_provider_errors_total{{stage=\"{}\",category=\"{}\"}} {}",
                operation.as_str(),
                category.as_str(),
                metrics.r2_provider_errors
                    [operation_index(operation) * 3 + provider_error_index(category)]
            )
            .ok();
        }
    }
    write_help(
        out,
        "r2_condition_failures_total",
        "counter",
        "R2 condition failures",
    );
    for (index, operation) in ["get", "put"].into_iter().enumerate() {
        writeln!(
            out,
            "r2_condition_failures_total{{operation=\"{operation}\"}} {}",
            metrics.r2_condition_failures[index]
        )
        .ok();
    }
    write_help(
        out,
        "r2_list_head_fanout_total",
        "counter",
        "R2 list metadata HEAD fanout",
    );
    writeln!(
        out,
        "r2_list_head_fanout_total {}",
        metrics.r2_list_head_fanout
    )
    .ok();
    write_help(
        out,
        "r2_result_unknown_total",
        "counter",
        "R2 mutation result-unknown outcomes",
    );
    for (index, operation) in ["put", "delete"].into_iter().enumerate() {
        writeln!(
            out,
            "r2_result_unknown_total{{operation=\"{operation}\"}} {}",
            metrics.r2_result_unknown[index]
        )
        .ok();
    }
    write_help(
        out,
        "r2_force_delete_remaining_batches",
        "gauge",
        "Remaining R2 force-delete batches",
    );
    writeln!(
        out,
        "r2_force_delete_remaining_batches {}",
        metrics.r2_force_delete_remaining_batches
    )
    .ok();
}

pub(super) fn operation_index(operation: R2Operation) -> usize {
    operations()
        .iter()
        .position(|candidate| *candidate == operation)
        .unwrap()
}

fn provider_error_index(error: R2ProviderError) -> usize {
    provider_errors()
        .iter()
        .position(|candidate| *candidate == error)
        .unwrap()
}

impl super::MetricsRegistry {
    pub(crate) fn observe_r2_operation(
        &self,
        operation: R2Operation,
        success: bool,
        duration: Duration,
    ) {
        let index = operation_index(operation);
        let mut guard = self.lock();
        guard.r2_operations[index * 2 + usize::from(success)] =
            guard.r2_operations[index * 2 + usize::from(success)].saturating_add(1);
        guard.r2_operation_duration[index] = duration.as_secs_f64();
    }

    pub(crate) fn inc_r2_provider_error(&self, operation: R2Operation, category: R2ProviderError) {
        let index = operation_index(operation) * 3 + provider_error_index(category);
        let mut guard = self.lock();
        guard.r2_provider_errors[index] = guard.r2_provider_errors[index].saturating_add(1);
    }

    pub(crate) fn add_r2_bytes(&self, direction: R2StreamDirection, bytes: u64) {
        let index = match direction {
            R2StreamDirection::Upload => 0,
            R2StreamDirection::Download => 1,
        };
        let mut guard = self.lock();
        guard.r2_bytes[index] = guard.r2_bytes[index].saturating_add(bytes);
    }

    pub(crate) fn adjust_r2_staging_bytes(&self, bytes: u64, increase: bool) {
        let mut guard = self.lock();
        guard.r2_staging_bytes = if increase {
            guard.r2_staging_bytes.saturating_add(bytes)
        } else {
            guard.r2_staging_bytes.saturating_sub(bytes)
        };
    }

    pub(crate) fn inc_r2_condition_failure(&self, put: bool) {
        let mut guard = self.lock();
        let index = usize::from(put);
        guard.r2_condition_failures[index] = guard.r2_condition_failures[index].saturating_add(1);
    }

    pub(crate) fn add_r2_list_head_fanout(&self, count: u64) {
        let mut guard = self.lock();
        guard.r2_list_head_fanout = guard.r2_list_head_fanout.saturating_add(count);
    }

    pub(crate) fn inc_r2_result_unknown(&self, delete: bool) {
        let mut guard = self.lock();
        let index = usize::from(delete);
        guard.r2_result_unknown[index] = guard.r2_result_unknown[index].saturating_add(1);
    }

    pub(crate) fn set_r2_force_delete_remaining_batches(&self, batches: u64) {
        self.lock().r2_force_delete_remaining_batches = batches;
    }

    fn adjust_r2_stream(&self, direction: R2StreamDirection, increase: bool) {
        let index = match direction {
            R2StreamDirection::Upload => 0,
            R2StreamDirection::Download => 1,
        };
        let mut guard = self.lock();
        guard.r2_active_streams[index] = if increase {
            guard.r2_active_streams[index].saturating_add(1)
        } else {
            guard.r2_active_streams[index].saturating_sub(1)
        };
    }
}

fn operations() -> [R2Operation; 5] {
    [
        R2Operation::Delete,
        R2Operation::Get,
        R2Operation::Head,
        R2Operation::List,
        R2Operation::Put,
    ]
}

fn provider_errors() -> [R2ProviderError; 3] {
    [
        R2ProviderError::Availability,
        R2ProviderError::Integrity,
        R2ProviderError::ResultUnknown,
    ]
}

const fn outcome(success: bool) -> &'static str {
    if success { "success" } else { "failure" }
}
