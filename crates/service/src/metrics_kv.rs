//! Fixed low-cardinality P0.4 KV metrics.

use super::{Inner, write_help};
use std::fmt::Write as _;
use std::sync::Arc;

#[derive(Clone, Copy)]
pub(crate) enum KvGauge {
    ReaderConnection,
    WriterConnection,
    ActiveStream,
}

pub(crate) struct KvGaugeGuard {
    metrics: Arc<super::MetricsRegistry>,
    gauge: KvGauge,
}

impl KvGaugeGuard {
    pub(crate) fn new(metrics: &Arc<super::MetricsRegistry>, gauge: KvGauge) -> Self {
        metrics.adjust_kv_gauge(gauge, true);
        Self {
            metrics: metrics.clone(),
            gauge,
        }
    }
}

impl Drop for KvGaugeGuard {
    fn drop(&mut self) {
        self.metrics.adjust_kv_gauge(self.gauge, false);
    }
}

pub(crate) struct KvStagingGauge {
    metrics: Option<Arc<super::MetricsRegistry>>,
    bytes: u64,
    _active: Option<KvGaugeGuard>,
}

impl std::fmt::Debug for KvStagingGauge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KvStagingGauge")
            .field("bytes", &self.bytes)
            .finish_non_exhaustive()
    }
}

impl KvStagingGauge {
    pub(crate) fn new(metrics: Option<&Arc<super::MetricsRegistry>>) -> Self {
        Self {
            metrics: metrics.cloned(),
            bytes: 0,
            _active: metrics.map(|metrics| KvGaugeGuard::new(metrics, KvGauge::ActiveStream)),
        }
    }

    pub(crate) fn add(&mut self, bytes: usize) {
        let bytes = u64::try_from(bytes).unwrap_or(u64::MAX);
        self.bytes = self.bytes.saturating_add(bytes);
        if let Some(metrics) = &self.metrics {
            metrics.adjust_kv_staging_bytes(bytes, true);
        }
    }
}

impl Drop for KvStagingGauge {
    fn drop(&mut self) {
        if let Some(metrics) = &self.metrics {
            metrics.adjust_kv_staging_bytes(self.bytes, false);
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum KvOperation {
    Get,
    GetWithMetadata,
    GetMany,
    Put,
    Delete,
    List,
}

impl KvOperation {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Get => "get",
            Self::GetWithMetadata => "get_with_metadata",
            Self::GetMany => "get_many",
            Self::Put => "put",
            Self::Delete => "delete",
            Self::List => "list",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum KvLifecycle {
    Backup,
    Restore,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum KvMaintenance {
    Gc,
    Checkpoint,
}

pub(crate) struct KvLifecycleGuard {
    metrics: Arc<super::MetricsRegistry>,
    lifecycle: KvLifecycle,
    successful: bool,
}

impl KvLifecycleGuard {
    pub(crate) fn new(metrics: Arc<super::MetricsRegistry>, lifecycle: KvLifecycle) -> Self {
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

impl Drop for KvLifecycleGuard {
    fn drop(&mut self) {
        self.metrics
            .inc_kv_lifecycle(self.lifecycle, self.successful);
    }
}

pub(super) fn write_kv_metrics(out: &mut String, metrics: &Inner) {
    write_help(
        out,
        "kv_operations_total",
        "counter",
        "KV binding operation outcomes",
    );
    for operation in operations() {
        let index = operation_index(operation);
        for success in [false, true] {
            writeln!(
                out,
                "kv_operations_total{{operation=\"{}\",outcome=\"{}\",type=\"raw\"}} {}",
                operation.as_str(),
                outcome(success),
                metrics.kv_operations[index * 2 + usize::from(success)]
            )
            .ok();
        }
    }
    write_help(
        out,
        "kv_operation_duration_seconds",
        "gauge",
        "Last KV binding operation duration",
    );
    for operation in operations() {
        writeln!(
            out,
            "kv_operation_duration_seconds{{operation=\"{}\"}} {}",
            operation.as_str(),
            metrics.kv_operation_duration[operation_index(operation)]
        )
        .ok();
    }
    write_help(
        out,
        "kv_operation_bytes",
        "counter",
        "KV binding operation bytes",
    );
    for operation in operations() {
        let index = operation_index(operation) * 2;
        writeln!(
            out,
            "kv_operation_bytes{{operation=\"{}\",direction=\"ingress\"}} {}",
            operation.as_str(),
            metrics.kv_operation_bytes[index]
        )
        .ok();
        writeln!(
            out,
            "kv_operation_bytes{{operation=\"{}\",direction=\"egress\"}} {}",
            operation.as_str(),
            metrics.kv_operation_bytes[index + 1]
        )
        .ok();
    }
    write_help(out, "kv_open_connections", "gauge", "Open KV connections");
    writeln!(
        out,
        "kv_open_connections{{role=\"reader\"}} {}",
        metrics.kv_open_connections[0]
    )
    .ok();
    writeln!(
        out,
        "kv_open_connections{{role=\"writer\"}} {}",
        metrics.kv_open_connections[1]
    )
    .ok();
    write_help(out, "kv_active_streams", "gauge", "Active KV value streams");
    writeln!(out, "kv_active_streams {}", metrics.kv_active_streams).ok();
    write_help(out, "kv_staging_bytes", "gauge", "KV staging bytes");
    writeln!(out, "kv_staging_bytes {}", metrics.kv_staging_bytes).ok();
    write_help(
        out,
        "kv_wal_bytes_bucket",
        "counter",
        "Observed KV WAL bytes",
    );
    for (index, upper) in [1_u64 << 20, 4 << 20, 16 << 20, 64 << 20, u64::MAX]
        .into_iter()
        .enumerate()
    {
        let label = if upper == u64::MAX {
            "+Inf".to_owned()
        } else {
            upper.to_string()
        };
        writeln!(
            out,
            "kv_wal_bytes_bucket{{le=\"{label}\"}} {}",
            metrics.kv_wal_bytes[index]
        )
        .ok();
    }
    write_outcome_pair(
        out,
        "kv_gc_entries_total",
        "KV expiration GC outcomes",
        metrics.kv_gc,
    );
    write_outcome_pair(
        out,
        "kv_checkpoint_total",
        "KV checkpoint outcomes",
        metrics.kv_checkpoint,
    );
    write_outcome_pair(
        out,
        "kv_backup_total",
        "KV backup lifecycle outcomes",
        metrics.kv_backup,
    );
    write_outcome_pair(
        out,
        "kv_restore_total",
        "KV restore outcomes",
        metrics.kv_restore,
    );
    write_help(
        out,
        "kv_corruption_total",
        "counter",
        "KV isolated corruption outcomes",
    );
    for (index, class) in ["identity", "manifest", "sqlite"].into_iter().enumerate() {
        writeln!(
            out,
            "kv_corruption_total{{class=\"{class}\"}} {}",
            metrics.kv_corruption[index]
        )
        .ok();
    }
}

fn write_outcome_pair(out: &mut String, name: &str, help: &str, values: [u64; 2]) {
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

pub(super) fn operation_index(operation: KvOperation) -> usize {
    operations()
        .iter()
        .position(|candidate| *candidate == operation)
        .unwrap()
}

pub(super) fn lifecycle_index(lifecycle: KvLifecycle) -> usize {
    match lifecycle {
        KvLifecycle::Backup => 0,
        KvLifecycle::Restore => 1,
    }
}

pub(super) fn maintenance_index(maintenance: KvMaintenance) -> usize {
    match maintenance {
        KvMaintenance::Gc => 0,
        KvMaintenance::Checkpoint => 1,
    }
}

impl super::MetricsRegistry {
    fn adjust_kv_gauge(&self, gauge: KvGauge, increase: bool) {
        let mut guard = self.lock();
        let value = match gauge {
            KvGauge::ReaderConnection => &mut guard.kv_open_connections[0],
            KvGauge::WriterConnection => &mut guard.kv_open_connections[1],
            KvGauge::ActiveStream => &mut guard.kv_active_streams,
        };
        *value = if increase {
            value.saturating_add(1)
        } else {
            value.saturating_sub(1)
        };
    }

    fn adjust_kv_staging_bytes(&self, bytes: u64, increase: bool) {
        let mut guard = self.lock();
        guard.kv_staging_bytes = if increase {
            guard.kv_staging_bytes.saturating_add(bytes)
        } else {
            guard.kv_staging_bytes.saturating_sub(bytes)
        };
    }

    pub(crate) fn observe_kv_wal_bytes(&self, bytes: u64) {
        let mut guard = self.lock();
        for (index, upper) in [1_u64 << 20, 4 << 20, 16 << 20, 64 << 20, u64::MAX]
            .into_iter()
            .enumerate()
        {
            if bytes <= upper {
                guard.kv_wal_bytes[index] = guard.kv_wal_bytes[index].saturating_add(1);
            }
        }
    }
}

fn operations() -> [KvOperation; 6] {
    [
        KvOperation::Delete,
        KvOperation::Get,
        KvOperation::GetMany,
        KvOperation::GetWithMetadata,
        KvOperation::List,
        KvOperation::Put,
    ]
}

const fn outcome(success: bool) -> &'static str {
    if success { "success" } else { "failure" }
}
