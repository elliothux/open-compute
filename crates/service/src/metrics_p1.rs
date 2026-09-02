//! Fixed-series P1 hardening, recovery, and release metrics.

use super::{MetricsRegistry, escape, write_help};
use open_compute_core::{AdmissionSnapshotV1, ErrorCode, OperationClass, PlatformError};
use std::fmt::Write as _;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Low-cardinality WebSocket terminal reason observed by the platform bridge.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum WebSocketCloseReason {
    /// Peer completed an ordinary close.
    Normal,
    /// Version generation was replaced.
    VersionRestart,
    /// Platform or runtime shutdown terminated the tunnel.
    Shutdown,
    /// Transport or protocol error terminated the tunnel.
    Error,
    /// No close frame or more specific reason was observable.
    Disconnected,
}

impl WebSocketCloseReason {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::VersionRestart => "version_restart",
            Self::Shutdown => "shutdown",
            Self::Error => "error",
            Self::Disconnected => "disconnected",
        }
    }
}

#[derive(Debug)]
pub(super) struct P1Metrics {
    admission_total: [u64; 40],
    disk_free_bytes: u64,
    disk_reserved_bytes: u64,
    disk_staging_bytes: u64,
    disk_emergency_headroom_bytes: u64,
    resource_count: [u64; 11],
    quota_reject_total: [u64; 5],
    snapshot_last_bytes: u64,
    snapshot_last_duration: f64,
    snapshot_last_success_ms: i64,
    snapshot_success_total: u64,
    snapshot_inspect_failure_total: u64,
    restore_last_completed_ms: i64,
    restore_last_duration: f64,
    restore_last_smoke_verified: u64,
    schema_current: u64,
    schema_failed_resources: u64,
    sqlite_busy_total: u64,
    sqlite_check_failure_total: u64,
    websocket_close: [u64; 5],
    release_lock_sha256: String,
    conformance_result: String,
}

impl Default for P1Metrics {
    fn default() -> Self {
        Self {
            admission_total: [0; 40],
            disk_free_bytes: 0,
            disk_reserved_bytes: 0,
            disk_staging_bytes: 0,
            disk_emergency_headroom_bytes: 0,
            resource_count: [0; 11],
            quota_reject_total: [0; 5],
            snapshot_last_bytes: 0,
            snapshot_last_duration: 0.0,
            snapshot_last_success_ms: 0,
            snapshot_success_total: 0,
            snapshot_inspect_failure_total: 0,
            restore_last_completed_ms: 0,
            restore_last_duration: 0.0,
            restore_last_smoke_verified: 0,
            schema_current: 0,
            schema_failed_resources: 0,
            sqlite_busy_total: 0,
            sqlite_check_failure_total: 0,
            websocket_close: [0; 5],
            release_lock_sha256: "unknown".to_owned(),
            conformance_result: "unknown".to_owned(),
        }
    }
}

impl MetricsRegistry {
    /// Record one centralized admission outcome using fixed low-cardinality labels.
    pub fn observe_admission(&self, operation: OperationClass, error: Option<ErrorCode>) {
        let outcome = match error {
            None => 0,
            Some(ErrorCode::QuotaExceeded) => 1,
            Some(ErrorCode::AdmissionBusy) => 2,
            Some(ErrorCode::StoragePressure | ErrorCode::DiskHardLimit) => 3,
            Some(_) => 4,
        };
        let mut guard = self.lock();
        let index = admission_operation_index(operation) * 5 + outcome;
        guard.p1.admission_total[index] = guard.p1.admission_total[index].saturating_add(1);
    }

    /// Publish one immutable disk-admission snapshot and emergency headroom.
    pub fn set_disk_admission(&self, snapshot: &AdmissionSnapshotV1, emergency_reserve: u64) {
        let mut guard = self.lock();
        guard.p1.disk_free_bytes = snapshot.filesystem_free_bytes;
        guard.p1.disk_reserved_bytes = snapshot.reserved_bytes;
        guard.p1.disk_staging_bytes = snapshot.owned_staging_bytes;
        guard.p1.disk_emergency_headroom_bytes = snapshot
            .filesystem_free_bytes
            .saturating_sub(snapshot.reserved_bytes)
            .saturating_sub(emergency_reserve);
    }

    /// Publish fixed aggregate control-authority counts without tenant labels.
    pub fn set_resource_counts(&self, values: [u64; 11]) {
        self.lock().p1.resource_count = values;
    }

    /// Record a completed offline snapshot receipt when loaded at startup.
    pub fn record_snapshot_receipt(&self, bytes: u64, duration: Duration) {
        self.record_snapshot_receipt_at(bytes, duration, unix_ms());
    }

    /// Record a completed offline snapshot receipt with its audit timestamp.
    pub fn record_snapshot_receipt_at(&self, bytes: u64, duration: Duration, completed_at_ms: i64) {
        let mut guard = self.lock();
        guard.p1.snapshot_last_bytes = bytes;
        guard.p1.snapshot_last_duration = duration.as_secs_f64();
        guard.p1.snapshot_last_success_ms = completed_at_ms.max(0);
        guard.p1.snapshot_success_total = guard.p1.snapshot_success_total.saturating_add(1);
    }

    /// Record the most recent fresh-host restore receipt loaded during startup.
    pub fn record_restore_receipt(
        &self,
        completed_at_ms: i64,
        duration: Duration,
        smoke_verified: bool,
    ) {
        let mut guard = self.lock();
        guard.p1.restore_last_completed_ms = completed_at_ms.max(0);
        guard.p1.restore_last_duration = duration.as_secs_f64();
        guard.p1.restore_last_smoke_verified = u64::from(smoke_verified);
    }

    /// Record that a snapshot receipt or manifest sample could not be inspected.
    pub fn inc_snapshot_inspect_failure(&self) {
        let mut guard = self.lock();
        guard.p1.snapshot_inspect_failure_total =
            guard.p1.snapshot_inspect_failure_total.saturating_add(1);
    }

    /// Publish the verified current control schema version.
    pub fn set_schema_version(&self, current: u64) {
        let mut guard = self.lock();
        guard.p1.schema_current = current;
    }

    /// Publish the number of project resource files that failed schema inspection.
    pub fn set_schema_failed_resources(&self, failed: u64) {
        self.lock().p1.schema_failed_resources = failed;
    }

    /// Record a bounded SQLite busy outcome.
    pub fn inc_sqlite_busy(&self) {
        let mut guard = self.lock();
        guard.p1.sqlite_busy_total = guard.p1.sqlite_busy_total.saturating_add(1);
    }

    /// Record a project-owned SQLite integrity-check failure.
    pub fn inc_sqlite_check_failure(&self) {
        let mut guard = self.lock();
        guard.p1.sqlite_check_failure_total = guard.p1.sqlite_check_failure_total.saturating_add(1);
    }

    /// Record one WebSocket terminal reason without object or tenant labels.
    pub fn inc_websocket_close(&self, reason: WebSocketCloseReason) {
        let mut guard = self.lock();
        let index = websocket_close_index(reason);
        guard.p1.websocket_close[index] = guard.p1.websocket_close[index].saturating_add(1);
    }

    /// Bind the metrics snapshot to the exact runtime lock and conformance result.
    pub fn set_release_identity(
        &self,
        workerd_lock_sha256: &str,
        conformance_result: &str,
    ) -> Result<(), PlatformError> {
        if workerd_lock_sha256.len() != 64
            || !workerd_lock_sha256
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
            || workerd_lock_sha256.len() as u64 > self.max_label
            || conformance_result.is_empty()
            || conformance_result.len() as u64 > self.max_label
            || !conformance_result
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        {
            return Err(PlatformError::new(
                ErrorCode::LimitInvalid,
                "release identity metric label is invalid",
            ));
        }
        let mut guard = self.lock();
        guard.p1.release_lock_sha256 = workerd_lock_sha256.to_owned();
        guard.p1.conformance_result = conformance_result.to_owned();
        Ok(())
    }

    /// Increment one fixed product quota rejection counter.
    pub fn inc_quota_reject(&self, product: &str) {
        let Some(index) = ["workers", "kv", "r2", "d1", "durable_objects"]
            .iter()
            .position(|candidate| *candidate == product)
        else {
            return;
        };
        let mut guard = self.lock();
        guard.p1.quota_reject_total[index] = guard.p1.quota_reject_total[index].saturating_add(1);
    }

    /// Record stable product errors that contribute to cross-product P1 counters.
    pub fn observe_product_error(&self, operation: OperationClass, error: ErrorCode) {
        if error == ErrorCode::QuotaExceeded {
            let product = match operation {
                OperationClass::Workers => Some("workers"),
                OperationClass::Kv => Some("kv"),
                OperationClass::R2 => Some("r2"),
                OperationClass::D1 => Some("d1"),
                OperationClass::DurableObjects => Some("durable_objects"),
                OperationClass::Scheduler | OperationClass::Snapshot | OperationClass::Restore => {
                    None
                }
            };
            if let Some(product) = product {
                self.inc_quota_reject(product);
            }
        }
        if matches!(error, ErrorCode::KvBusy | ErrorCode::D1Overloaded) {
            self.inc_sqlite_busy();
        }
    }
}

pub(super) fn write_p1_metrics(out: &mut String, metrics: &P1Metrics) {
    write_help(
        out,
        "platform_release_info",
        "gauge",
        "Exact runtime lock and conformance result identity",
    );
    writeln!(
        out,
        "platform_release_info{{workerd_lock_sha256=\"{}\",conformance_result=\"{}\"}} 1",
        escape(&metrics.release_lock_sha256),
        escape(&metrics.conformance_result)
    )
    .ok();
    write_help(
        out,
        "platform_admission_total",
        "counter",
        "Central mutation admission outcomes",
    );
    for operation in operations() {
        for (outcome_index, outcome) in [
            "accepted",
            "quota",
            "busy",
            "storage_pressure",
            "unavailable",
        ]
        .iter()
        .enumerate()
        {
            writeln!(
                out,
                "platform_admission_total{{operation=\"{}\",outcome=\"{}\"}} {}",
                admission_operation_name(operation),
                outcome,
                metrics.admission_total[admission_operation_index(operation) * 5 + outcome_index]
            )
            .ok();
        }
    }
    for (name, value, kind) in [
        ("platform_disk_free_bytes", metrics.disk_free_bytes, "gauge"),
        (
            "platform_disk_reserved_bytes",
            metrics.disk_reserved_bytes,
            "gauge",
        ),
        (
            "platform_disk_staging_bytes",
            metrics.disk_staging_bytes,
            "gauge",
        ),
        (
            "platform_disk_emergency_headroom_bytes",
            metrics.disk_emergency_headroom_bytes,
            "gauge",
        ),
        (
            "platform_snapshot_last_bytes",
            metrics.snapshot_last_bytes,
            "gauge",
        ),
        (
            "platform_snapshot_success_total",
            metrics.snapshot_success_total,
            "counter",
        ),
        (
            "platform_snapshot_inspect_failure_total",
            metrics.snapshot_inspect_failure_total,
            "counter",
        ),
        (
            "platform_restore_last_smoke_verified",
            metrics.restore_last_smoke_verified,
            "gauge",
        ),
        ("platform_schema_current", metrics.schema_current, "gauge"),
        (
            "platform_schema_failed_resources",
            metrics.schema_failed_resources,
            "gauge",
        ),
        ("sqlite_busy_total", metrics.sqlite_busy_total, "counter"),
        (
            "sqlite_check_failure_total",
            metrics.sqlite_check_failure_total,
            "counter",
        ),
    ] {
        writeln!(out, "# TYPE {name} {kind}").ok();
        writeln!(out, "{name} {value}").ok();
    }
    write_help(
        out,
        "platform_resource_count",
        "gauge",
        "Aggregate live control-authority objects",
    );
    for (index, resource) in [
        "accounts",
        "workers",
        "versions",
        "routes",
        "kv_namespaces",
        "r2_buckets",
        "d1_databases",
        "do_namespaces",
        "vectorize_indexes",
        "ai_search_namespaces",
        "ai_search_instances",
    ]
    .into_iter()
    .enumerate()
    {
        writeln!(
            out,
            "platform_resource_count{{resource=\"{resource}\"}} {}",
            metrics.resource_count[index]
        )
        .ok();
    }
    writeln!(out, "# TYPE platform_snapshot_last_duration_seconds gauge").ok();
    writeln!(
        out,
        "platform_snapshot_last_duration_seconds {}",
        metrics.snapshot_last_duration
    )
    .ok();
    for (name, value) in [
        (
            "platform_snapshot_last_success_age_seconds",
            age_seconds(metrics.snapshot_last_success_ms),
        ),
        (
            "platform_restore_receipt_age_seconds",
            age_seconds(metrics.restore_last_completed_ms),
        ),
        (
            "platform_restore_last_duration_seconds",
            metrics.restore_last_duration,
        ),
    ] {
        writeln!(out, "# TYPE {name} gauge").ok();
        writeln!(out, "{name} {value}").ok();
    }
    write_help(
        out,
        "oc_do_websocket_close_total",
        "counter",
        "Durable Object WebSocket terminal reason classes",
    );
    for reason in websocket_close_reasons() {
        writeln!(
            out,
            "oc_do_websocket_close_total{{reason=\"{}\"}} {}",
            reason.as_str(),
            metrics.websocket_close[websocket_close_index(reason)]
        )
        .ok();
    }
    for (index, product) in ["workers", "kv", "r2", "d1", "durable_objects"]
        .iter()
        .enumerate()
    {
        writeln!(
            out,
            "platform_quota_reject_total{{product=\"{}\"}} {}",
            product, metrics.quota_reject_total[index]
        )
        .ok();
    }
}

const fn operations() -> [OperationClass; 8] {
    [
        OperationClass::Workers,
        OperationClass::Kv,
        OperationClass::R2,
        OperationClass::D1,
        OperationClass::DurableObjects,
        OperationClass::Scheduler,
        OperationClass::Snapshot,
        OperationClass::Restore,
    ]
}

const fn admission_operation_index(operation: OperationClass) -> usize {
    match operation {
        OperationClass::Workers => 0,
        OperationClass::Kv => 1,
        OperationClass::R2 => 2,
        OperationClass::D1 => 3,
        OperationClass::DurableObjects => 4,
        OperationClass::Scheduler => 5,
        OperationClass::Snapshot => 6,
        OperationClass::Restore => 7,
    }
}

const fn admission_operation_name(operation: OperationClass) -> &'static str {
    match operation {
        OperationClass::Workers => "workers",
        OperationClass::Kv => "kv",
        OperationClass::R2 => "r2",
        OperationClass::D1 => "d1",
        OperationClass::DurableObjects => "durable_objects",
        OperationClass::Scheduler => "scheduler",
        OperationClass::Snapshot => "snapshot",
        OperationClass::Restore => "restore",
    }
}

const fn websocket_close_reasons() -> [WebSocketCloseReason; 5] {
    [
        WebSocketCloseReason::Normal,
        WebSocketCloseReason::VersionRestart,
        WebSocketCloseReason::Shutdown,
        WebSocketCloseReason::Error,
        WebSocketCloseReason::Disconnected,
    ]
}

fn websocket_close_index(reason: WebSocketCloseReason) -> usize {
    websocket_close_reasons()
        .iter()
        .position(|candidate| *candidate == reason)
        .unwrap()
}

fn unix_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
        .unwrap_or(i64::MAX)
}

fn age_seconds(completed_at_ms: i64) -> f64 {
    if completed_at_ms <= 0 {
        return 0.0;
    }
    unix_ms().saturating_sub(completed_at_ms) as f64 / 1_000.0
}
