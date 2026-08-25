//! Fixed low-cardinality P0.6 D1 metrics.

use super::{Inner, write_help};
use std::fmt::Write as _;
use std::sync::Arc;
use std::time::Duration;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum D1Operation {
    Query,
    Batch,
    Exec,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum D1Lifecycle {
    Backup,
    Migration,
}

pub(crate) struct D1LifecycleGuard {
    metrics: Arc<super::MetricsRegistry>,
    lifecycle: D1Lifecycle,
    successful: bool,
}

impl D1LifecycleGuard {
    pub(crate) fn new(metrics: Arc<super::MetricsRegistry>, lifecycle: D1Lifecycle) -> Self {
        Self {
            metrics,
            lifecycle,
            successful: false,
        }
    }

    pub(crate) fn success(mut self) {
        self.successful = true;
    }
}

impl Drop for D1LifecycleGuard {
    fn drop(&mut self) {
        self.metrics
            .inc_d1_lifecycle(self.lifecycle, self.successful);
    }
}

impl D1Operation {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Query => "query",
            Self::Batch => "batch",
            Self::Exec => "exec",
        }
    }
}

pub(super) fn operation_index(operation: D1Operation) -> usize {
    operations()
        .iter()
        .position(|candidate| *candidate == operation)
        .unwrap()
}

pub(super) fn write_d1_metrics(out: &mut String, metrics: &Inner) {
    write_help(
        out,
        "d1_operations_total",
        "counter",
        "D1 operation outcomes",
    );
    for operation in operations() {
        let base = operation_index(operation) * 4;
        for readonly in [false, true] {
            for success in [false, true] {
                writeln!(
                    out,
                    "d1_operations_total{{operation=\"{}\",outcome=\"{}\",readonly=\"{}\"}} {}",
                    operation.as_str(),
                    outcome(success),
                    readonly,
                    metrics.d1_operations[base + usize::from(readonly) * 2 + usize::from(success)]
                )
                .ok();
            }
        }
    }
    write_help(
        out,
        "d1_operation_duration_seconds",
        "gauge",
        "Last D1 operation duration",
    );
    write_operation_values(
        out,
        "d1_operation_duration_seconds",
        "stage=\"total\"",
        &metrics.d1_operation_duration,
    );
    write_help(
        out,
        "d1_statement_duration_seconds",
        "gauge",
        "Last D1 statement duration",
    );
    for (index, kind) in ["readonly", "write", "ddl"].into_iter().enumerate() {
        writeln!(
            out,
            "d1_statement_duration_seconds{{kind=\"{kind}\"}} {}",
            metrics.d1_statement_duration[index]
        )
        .ok();
    }
    write_help(out, "d1_rows_output_total", "counter", "D1 output rows");
    write_operation_values(out, "d1_rows_output_total", "", &metrics.d1_rows_output);
    write_help(out, "d1_rows_written_total", "counter", "D1 written rows");
    write_operation_values(out, "d1_rows_written_total", "", &metrics.d1_rows_written);
    write_help(out, "d1_result_bytes_total", "counter", "D1 result bytes");
    write_operation_values(out, "d1_result_bytes_total", "", &metrics.d1_result_bytes);
    write_help(
        out,
        "d1_operation_queue_depth_bucket",
        "counter",
        "Observed D1 queue depth",
    );
    write_buckets(
        out,
        "d1_operation_queue_depth_bucket",
        &metrics.d1_queue_depth,
        [0, 1, 4, 16, u64::MAX],
    );
    write_help(
        out,
        "d1_open_databases",
        "gauge",
        "Active D1 database lanes",
    );
    writeln!(out, "d1_open_databases {}", metrics.d1_open_databases).ok();
    write_help(
        out,
        "d1_wal_bytes_bucket",
        "counter",
        "Observed D1 WAL bytes",
    );
    write_buckets(
        out,
        "d1_wal_bytes_bucket",
        &metrics.d1_wal_bytes,
        [1 << 20, 4 << 20, 16 << 20, 64 << 20, u64::MAX],
    );
    write_help(
        out,
        "d1_interrupts_total",
        "counter",
        "D1 interrupt reasons",
    );
    for (index, reason) in ["vm_steps", "timeout", "shutdown"].into_iter().enumerate() {
        writeln!(
            out,
            "d1_interrupts_total{{reason=\"{reason}\"}} {}",
            metrics.d1_interrupts[index]
        )
        .ok();
    }
    write_help(
        out,
        "d1_authorizer_denials_total",
        "counter",
        "D1 authorizer denial categories",
    );
    for (index, category) in ["connection", "filesystem", "internal", "extension"]
        .into_iter()
        .enumerate()
    {
        writeln!(
            out,
            "d1_authorizer_denials_total{{category=\"{category}\"}} {}",
            metrics.d1_authorizer_denials[index]
        )
        .ok();
    }
    write_help(
        out,
        "d1_result_unknown_total",
        "counter",
        "D1 result-unknown outcomes",
    );
    for (index, operation) in ["query", "batch", "exec", "migration"]
        .into_iter()
        .enumerate()
    {
        writeln!(
            out,
            "d1_result_unknown_total{{operation=\"{operation}\"}} {}",
            metrics.d1_result_unknown[index]
        )
        .ok();
    }
    write_outcomes(
        out,
        "d1_backup_total",
        "D1 backup outcomes",
        metrics.d1_backup,
    );
    write_outcomes(
        out,
        "d1_migration_total",
        "D1 migration outcomes",
        metrics.d1_migration,
    );
}

fn write_operation_values(
    out: &mut String,
    name: &str,
    extra: &str,
    values: &[impl std::fmt::Display; 3],
) {
    for operation in operations() {
        let comma = if extra.is_empty() { "" } else { "," };
        writeln!(
            out,
            "{name}{{operation=\"{}\"{comma}{extra}}} {}",
            operation.as_str(),
            values[operation_index(operation)]
        )
        .ok();
    }
}

fn write_buckets(out: &mut String, name: &str, values: &[u64; 5], bounds: [u64; 5]) {
    for (index, bound) in bounds.into_iter().enumerate() {
        let label = if bound == u64::MAX {
            "+Inf".to_owned()
        } else {
            bound.to_string()
        };
        writeln!(out, "{name}{{le=\"{label}\"}} {}", values[index]).ok();
    }
}

fn write_outcomes(out: &mut String, name: &str, help: &str, values: [u64; 2]) {
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

const fn operations() -> [D1Operation; 3] {
    [D1Operation::Query, D1Operation::Batch, D1Operation::Exec]
}

const fn outcome(success: bool) -> &'static str {
    if success { "success" } else { "failure" }
}

impl super::MetricsRegistry {
    pub(crate) fn observe_d1_queue_depth(&self, depth: u64) {
        let bounds = [0, 1, 4, 16, u64::MAX];
        let mut guard = self.lock();
        for (index, bound) in bounds.into_iter().enumerate() {
            if depth <= bound {
                guard.d1_queue_depth[index] = guard.d1_queue_depth[index].saturating_add(1);
            }
        }
    }

    pub(crate) fn set_d1_open_databases(&self, count: u64) {
        self.lock().d1_open_databases = count;
    }

    pub(crate) fn observe_d1_wal_bytes(&self, bytes: u64) {
        let bounds = [1 << 20, 4 << 20, 16 << 20, 64 << 20, u64::MAX];
        let mut guard = self.lock();
        for (index, bound) in bounds.into_iter().enumerate() {
            if bytes <= bound {
                guard.d1_wal_bytes[index] = guard.d1_wal_bytes[index].saturating_add(1);
            }
        }
    }

    pub(crate) fn inc_d1_lifecycle(&self, lifecycle: D1Lifecycle, success: bool) {
        let mut guard = self.lock();
        let values = match lifecycle {
            D1Lifecycle::Backup => &mut guard.d1_backup,
            D1Lifecycle::Migration => &mut guard.d1_migration,
        };
        values[usize::from(success)] = values[usize::from(success)].saturating_add(1);
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn observe_d1_operation(
        &self,
        operation: D1Operation,
        readonly: bool,
        success: bool,
        duration: Duration,
        rows_output: u64,
        rows_written: u64,
        result_bytes: u64,
    ) {
        let index = operation_index(operation);
        let mut guard = self.lock();
        let counter = index * 4 + usize::from(readonly) * 2 + usize::from(success);
        guard.d1_operations[counter] = guard.d1_operations[counter].saturating_add(1);
        guard.d1_operation_duration[index] = duration.as_secs_f64();
        guard.d1_statement_duration[usize::from(!readonly)] = duration.as_secs_f64();
        guard.d1_rows_output[index] = guard.d1_rows_output[index].saturating_add(rows_output);
        guard.d1_rows_written[index] = guard.d1_rows_written[index].saturating_add(rows_written);
        guard.d1_result_bytes[index] = guard.d1_result_bytes[index].saturating_add(result_bytes);
    }

    pub(crate) fn inc_d1_error(&self, operation: D1Operation, code: open_compute_core::ErrorCode) {
        let mut guard = self.lock();
        match code {
            open_compute_core::ErrorCode::D1Timeout
            | open_compute_core::ErrorCode::D1LimitError => {
                let index = usize::from(code == open_compute_core::ErrorCode::D1Timeout);
                guard.d1_interrupts[index] = guard.d1_interrupts[index].saturating_add(1);
            }
            open_compute_core::ErrorCode::D1AuthorizerDenied => {
                guard.d1_authorizer_denials[0] = guard.d1_authorizer_denials[0].saturating_add(1);
            }
            open_compute_core::ErrorCode::D1ResultUnknown => {
                let index = operation_index(operation);
                guard.d1_result_unknown[index] = guard.d1_result_unknown[index].saturating_add(1);
            }
            _ => {}
        }
    }
}
