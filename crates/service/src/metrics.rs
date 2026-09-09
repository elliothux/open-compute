//! Fixed, bounded metrics snapshot and Prometheus text rendering.

use open_compute_core::{
    ComponentName, ComponentState, ErrorCode, MetricsConfig, ObjectStorageKind, PlatformError,
    PlatformStatus,
};
use open_compute_runtime::supervisor::{SupervisorSnapshot, SupervisorState};
use std::fmt::Write as _;
use std::sync::Mutex;
use std::time::{Duration, Instant};

#[path = "metrics_cache_images.rs"]
mod cache_images;
#[path = "metrics_d1.rs"]
mod d1;
#[path = "metrics_do.rs"]
mod durable_objects;
#[path = "metrics_kv.rs"]
mod kv;
#[path = "metrics_p1.rs"]
mod p1;
#[path = "metrics_queue.rs"]
mod queue;
#[path = "metrics_r2.rs"]
mod r2;
#[path = "metrics_resource.rs"]
mod resource;
#[path = "metrics_scheduler.rs"]
mod scheduler;
#[path = "metrics_search.rs"]
mod search;
#[path = "metrics_service.rs"]
mod service;
#[path = "metrics_workflow.rs"]
mod workflow;
use cache_images::write_cache_images_metrics;
pub(crate) use cache_images::{
    CacheMetricOperation, CacheObjectOperation, ImageMetricOperation, ImageMetricOutcome,
};
use d1::write_d1_metrics;
pub(crate) use d1::{D1Lifecycle, D1LifecycleGuard, D1Operation};
use durable_objects::write_do_metrics;
pub(crate) use durable_objects::{DoFacetReloadReason, DoOperation, DoReconcileState};
use kv::write_kv_metrics;
pub(crate) use kv::{
    KvGauge, KvGaugeGuard, KvLifecycle, KvLifecycleGuard, KvMaintenance, KvOperation,
    KvStagingGauge,
};
pub use p1::WebSocketCloseReason;
use p1::{P1Metrics, write_p1_metrics};
use queue::write_queue_metrics;
pub(crate) use queue::{CronRunOutcome, QueueConsumerBatchOutcome};
pub(crate) use queue::{QueueMetricOperation, QueueReconcileOperation};
use r2::write_r2_metrics;
pub(crate) use r2::{R2Operation, R2ProviderError, R2StreamDirection, R2StreamGuard};
use resource::write_resource_metrics;
pub use resource::{BindingBackendOperation, ResourceOperation};
use scheduler::write_scheduler_metrics;
pub(crate) use scheduler::{AlarmMutation, AlarmOutcome, AlarmRepairSource, SchedulerClaimOutcome};
pub(crate) use search::{AiIndexStage, AiProviderCapability, AiProviderOutcome, AiSearchOperation};
use search::{SearchMetrics, write_search_metrics};
pub(crate) use service::ServiceMetricOperation;
use service::write_service_metrics;
pub(crate) use workflow::WorkflowOutcome;

/// Compile-time series required by the platform, product bindings, and P1 hardening surface.
pub const REQUIRED_SERIES: u64 = 747;
/// Longest compile-time label value (enum tokens). Runtime version strings must fit too.
pub const MIN_LABEL_VALUE_BYTES: u64 = 64;

/// Start outcome label.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum StartResult {
    /// Stage completed.
    Success,
    /// Stage failed.
    Failure,
}

impl StartResult {
    const ALL: [Self; 2] = [Self::Failure, Self::Success];

    const fn index(self) -> usize {
        match self {
            Self::Failure => 0,
            Self::Success => 1,
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Failure => "failure",
        }
    }
}

/// Startup stage label.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum StartStage {
    /// Config load.
    Config,
    /// Storage bootstrap.
    Storage,
    /// Runtime binary verify.
    RuntimeVerify,
    /// Object storage connect/preflight.
    ObjectStorage,
    /// Artifact cache open.
    Cache,
    /// Static config compile.
    Compile,
    /// Health listeners.
    Listen,
    /// Supervisor start.
    Supervisor,
}

impl StartStage {
    const ALL: [Self; 8] = [
        Self::Cache,
        Self::Compile,
        Self::Config,
        Self::Listen,
        Self::RuntimeVerify,
        Self::ObjectStorage,
        Self::Storage,
        Self::Supervisor,
    ];

    const fn index(self) -> usize {
        match self {
            Self::Cache => 0,
            Self::Compile => 1,
            Self::Config => 2,
            Self::Listen => 3,
            Self::RuntimeVerify => 4,
            Self::ObjectStorage => 5,
            Self::Storage => 6,
            Self::Supervisor => 7,
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::Config => "config",
            Self::Storage => "storage",
            Self::RuntimeVerify => "runtime_verify",
            Self::ObjectStorage => "object_storage",
            Self::Cache => "cache",
            Self::Compile => "compile",
            Self::Listen => "listen",
            Self::Supervisor => "supervisor",
        }
    }
}

/// workerd restart reason label.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum RestartReason {
    /// Child exited unexpectedly.
    UnexpectedExit,
    /// Authenticated probe failed.
    ProbeFailed,
    /// Operator/runtime unhealthy report.
    Unhealthy,
    /// Restart budget exhausted.
    BudgetExhausted,
}

impl RestartReason {
    const ALL: [Self; 4] = [
        Self::BudgetExhausted,
        Self::ProbeFailed,
        Self::UnexpectedExit,
        Self::Unhealthy,
    ];

    const fn index(self) -> usize {
        match self {
            Self::BudgetExhausted => 0,
            Self::ProbeFailed => 1,
            Self::UnexpectedExit => 2,
            Self::Unhealthy => 3,
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::UnexpectedExit => "unexpected_exit",
            Self::ProbeFailed => "probe_failed",
            Self::Unhealthy => "unhealthy",
            Self::BudgetExhausted => "budget_exhausted",
        }
    }
}

/// Control-db operation label.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum SqliteOp {
    /// Open.
    Open,
    /// Migrate.
    Migrate,
    /// Query.
    Query,
    /// Checkpoint.
    Checkpoint,
}

impl SqliteOp {
    const ALL: [Self; 4] = [Self::Checkpoint, Self::Migrate, Self::Open, Self::Query];

    const fn index(self) -> usize {
        match self {
            Self::Checkpoint => 0,
            Self::Migrate => 1,
            Self::Open => 2,
            Self::Query => 3,
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Migrate => "migrate",
            Self::Query => "query",
            Self::Checkpoint => "checkpoint",
        }
    }
}

/// Object-storage operation label.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum ObjectOp {
    /// PUT.
    Put,
    /// HEAD.
    Head,
    /// GET.
    Get,
    /// DELETE.
    Delete,
    /// LIST.
    List,
}

impl ObjectOp {
    const ALL: [Self; 5] = [Self::Delete, Self::Get, Self::Head, Self::List, Self::Put];

    const fn index(self) -> usize {
        match self {
            Self::Delete => 0,
            Self::Get => 1,
            Self::Head => 2,
            Self::List => 3,
            Self::Put => 4,
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::Put => "put",
            Self::Head => "head",
            Self::Get => "get",
            Self::Delete => "delete",
            Self::List => "list",
        }
    }
}

/// Object-storage result label.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum ObjectResult {
    /// Success.
    Success,
    /// Failure.
    Failure,
}

impl ObjectResult {
    const ALL: [Self; 2] = [Self::Failure, Self::Success];

    const fn index(self) -> usize {
        match self {
            Self::Failure => 0,
            Self::Success => 1,
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Failure => "failure",
        }
    }
}

#[derive(Debug)]
struct Inner {
    version: String,
    workerd_version: String,
    start_total: [u64; 16],
    restart_total: [u64; 4],
    process_up: u64,
    start_duration: f64,
    sqlite_duration: [f64; 4],
    object_backend: ObjectStorageKind,
    object_total: [u64; 10],
    object_duration: [f64; 5],
    cache_bytes: u64,
    cache_entries: u64,
    cache_hits: u64,
    integrity_errors: u64,
    resource_operations: [u64; 10],
    resource_duration: [f64; 5],
    resource_open_handles: u64,
    resource_pin_wait: f64,
    resource_reconcile: [u64; 4],
    binding_backend_requests: [u64; 6],
    binding_backend_bytes: [u64; 2],
    binding_protocol_errors: u64,
    kv_operations: [u64; 12],
    kv_operation_duration: [f64; 6],
    kv_operation_bytes: [u64; 12],
    kv_open_connections: [u64; 2],
    kv_active_streams: u64,
    kv_staging_bytes: u64,
    kv_wal_bytes: [u64; 5],
    kv_gc: [u64; 2],
    kv_checkpoint: [u64; 2],
    kv_backup: [u64; 2],
    kv_restore: [u64; 2],
    kv_corruption: [u64; 3],
    r2_operations: [u64; 10],
    r2_operation_duration: [f64; 5],
    r2_bytes: [u64; 2],
    r2_active_streams: [u64; 2],
    r2_staging_bytes: u64,
    r2_provider_errors: [u64; 15],
    r2_condition_failures: [u64; 2],
    r2_list_head_fanout: u64,
    r2_result_unknown: [u64; 2],
    r2_force_delete_remaining_batches: u64,
    d1_operations: [u64; 12],
    d1_operation_duration: [f64; 3],
    d1_statement_duration: [f64; 3],
    d1_rows_output: [u64; 3],
    d1_rows_written: [u64; 3],
    d1_result_bytes: [u64; 3],
    d1_queue_depth: [u64; 5],
    d1_open_databases: u64,
    d1_wal_bytes: [u64; 5],
    d1_interrupts: [u64; 3],
    d1_authorizer_denials: [u64; 4],
    d1_result_unknown: [u64; 4],
    d1_backup: [u64; 2],
    d1_restore: [u64; 2],
    d1_migration: [u64; 2],
    do_dispatch: [u64; 6],
    do_dispatch_duration: [f64; 3],
    do_active_hosts: u64,
    do_facet_reload: [u64; 3],
    do_reconcile: [u64; 4],
    do_storage_watermark: usize,
    scheduler_jobs: [u64; 3],
    scheduler_claim: [u64; 12],
    scheduler_dispatch_duration: [f64; 6],
    scheduler_claim_duration: [f64; 4],
    scheduler_oldest_due_age: [f64; 4],
    scheduler_ready: [u64; 4],
    scheduler_stale_completion: [u64; 4],
    scheduler_pool_state: [u8; 4],
    scheduler_wake: [u64; 5],
    scheduler_claim_expired: [u64; 4],
    scheduler_in_flight: [u64; 4],
    alarm_mutation: [u64; 6],
    alarm_delivery: [u64; 42],
    alarm_repair: [u64; 6],
    alarm_lag_seconds: f64,
    service_invocations: [u64; 10],
    service_invocation_duration: [f64; 5],
    service_roots: u64,
    service_operations: u64,
    service_retentions: u64,
    queue: queue::QueueMetrics,
    workflow: workflow::WorkflowMetrics,
    cache_images: cache_images::CacheImagesMetrics,
    search: SearchMetrics,
    observability_ingest: [u64; 2],
    observability_events: [u64; 6],
    observability_ingest_queue_depth: u64,
    observability_db_bytes: u64,
    observability_oldest_event_age_seconds: f64,
    observability_truncated: [u64; 2],
    observability_tail_sessions: u64,
    observability_tail_events: [u64; 2],
    observability_tail_dropped: [u64; 2],
    observability_query: [u64; 4],
    observability_query_duration_seconds: f64,
    last_supervisor: Option<SupervisorState>,
    last_attempt: Option<u32>,
    runtime_start: Option<Instant>,
    p1: P1Metrics,
}

mod recording;
mod render;

pub use recording::MetricsRegistry;

fn write_observability_metrics(out: &mut String, value: &Inner) {
    write_help(
        out,
        "open_compute_observability_ingest_total",
        "counter",
        "Collector ingest outcomes",
    );
    for (index, result) in ["failure", "success"].into_iter().enumerate() {
        writeln!(
            out,
            "open_compute_observability_ingest_total{{result=\"{result}\"}} {}",
            value.observability_ingest[index]
        )
        .ok();
    }
    write_help(
        out,
        "open_compute_observability_events_total",
        "counter",
        "Canonical event outcomes",
    );
    for (kind_index, kind) in ["invocation", "log", "exception"].into_iter().enumerate() {
        for (result_index, result) in ["dropped", "accepted"].into_iter().enumerate() {
            writeln!(
                out,
                "open_compute_observability_events_total{{kind=\"{kind}\",result=\"{result}\"}} {}",
                value.observability_events[kind_index * 2 + result_index]
            )
            .ok();
        }
    }
    write_help(
        out,
        "open_compute_observability_ingest_queue_depth",
        "gauge",
        "Invocation envelopes awaiting persistence",
    );
    writeln!(
        out,
        "open_compute_observability_ingest_queue_depth {}",
        value.observability_ingest_queue_depth
    )
    .ok();
    write_help(
        out,
        "open_compute_observability_db_bytes",
        "gauge",
        "Accounted observability database bytes",
    );
    writeln!(
        out,
        "open_compute_observability_db_bytes {}",
        value.observability_db_bytes
    )
    .ok();
    write_help(
        out,
        "open_compute_observability_oldest_event_age_seconds",
        "gauge",
        "Age of the oldest committed event",
    );
    writeln!(
        out,
        "open_compute_observability_oldest_event_age_seconds {}",
        value.observability_oldest_event_age_seconds
    )
    .ok();
    write_help(
        out,
        "open_compute_observability_truncated_total",
        "counter",
        "Observability projection truncations",
    );
    for (index, stage) in ["collector", "canonical"].into_iter().enumerate() {
        writeln!(
            out,
            "open_compute_observability_truncated_total{{stage=\"{stage}\"}} {}",
            value.observability_truncated[index]
        )
        .ok();
    }
    write_help(
        out,
        "open_compute_observability_tail_sessions",
        "gauge",
        "Current process-local Script Tail sessions",
    );
    writeln!(
        out,
        "open_compute_observability_tail_sessions {}",
        value.observability_tail_sessions
    )
    .ok();
    write_help(
        out,
        "open_compute_observability_tail_events_total",
        "counter",
        "Realtime event fan-out outcomes",
    );
    for (index, result) in ["filtered", "delivered"].into_iter().enumerate() {
        writeln!(
            out,
            "open_compute_observability_tail_events_total{{result=\"{result}\"}} {}",
            value.observability_tail_events[index]
        )
        .ok();
    }
    write_help(
        out,
        "open_compute_observability_tail_dropped_total",
        "counter",
        "Realtime event drop reasons",
    );
    for (index, reason) in ["closed", "overload"].into_iter().enumerate() {
        writeln!(
            out,
            "open_compute_observability_tail_dropped_total{{reason=\"{reason}\"}} {}",
            value.observability_tail_dropped[index]
        )
        .ok();
    }
    write_help(
        out,
        "open_compute_observability_query_total",
        "counter",
        "Telemetry query outcomes",
    );
    for (view_index, view) in ["events", "invocations"].into_iter().enumerate() {
        for (result_index, result) in ["failure", "success"].into_iter().enumerate() {
            writeln!(
                out,
                "open_compute_observability_query_total{{view=\"{view}\",result=\"{result}\"}} {}",
                value.observability_query[view_index * 2 + result_index]
            )
            .ok();
        }
    }
    write_help(
        out,
        "open_compute_observability_query_duration_seconds",
        "gauge",
        "Last telemetry query duration",
    );
    writeln!(
        out,
        "open_compute_observability_query_duration_seconds {}",
        value.observability_query_duration_seconds
    )
    .ok();
}

fn write_help(out: &mut String, name: &str, ty: &str, help: &str) {
    writeln!(out, "# HELP {name} {help}").ok();
    writeln!(out, "# TYPE {name} {ty}").ok();
}

fn escape(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('\n', "\\n")
        .replace('"', "\\\"")
}

fn component_order() -> [ComponentName; 14] {
    [
        ComponentName::Cache,
        ComponentName::ControlDb,
        ComponentName::DataDir,
        ComponentName::MasterKey,
        ComponentName::Operations,
        ComponentName::Process,
        ComponentName::Runtime,
        ComponentName::ObjectStorage,
        ComponentName::Scheduler,
        ComponentName::VectorizeStorage,
        ComponentName::VectorizeMutations,
        ComponentName::AiSearchStorage,
        ComponentName::AiSearchIndexing,
        ComponentName::AiModels,
    ]
}

fn start_index(result: StartResult, stage: StartStage) -> usize {
    result.index() * StartStage::ALL.len() + stage.index()
}

fn restart_index(reason: RestartReason) -> usize {
    reason.index()
}

fn sqlite_index(op: SqliteOp) -> usize {
    op.index()
}

fn object_op_index(op: ObjectOp) -> usize {
    op.index()
}

fn object_total_index(op: ObjectOp, result: ObjectResult) -> usize {
    op.index() * ObjectResult::ALL.len() + result.index()
}

const fn success_outcome(success: bool) -> &'static str {
    if success { "success" } else { "failure" }
}

/// Prometheus content type.
pub const CONTENT_TYPE: &str = "text/plain; version=0.0.4; charset=utf-8";
