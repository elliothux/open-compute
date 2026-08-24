//! Fixed, bounded metrics snapshot and Prometheus text rendering.

use open_compute_core::{
    ComponentName, ComponentState, ErrorCode, MetricsConfig, PlatformError, PlatformStatus,
};
use open_compute_runtime::supervisor::{SupervisorSnapshot, SupervisorState};
use std::fmt::Write as _;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Compile-time series required by section 14.2.
pub const REQUIRED_SERIES: u64 = 53;
/// Longest compile-time label value (enum tokens). Runtime version strings must fit too.
pub const MIN_LABEL_VALUE_BYTES: u64 = 32;

/// Start outcome label.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum StartResult {
    /// Stage completed.
    Success,
    /// Stage failed.
    Failure,
}

impl StartResult {
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
    /// S3 connect/preflight.
    S3,
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
    const fn as_str(self) -> &'static str {
        match self {
            Self::Config => "config",
            Self::Storage => "storage",
            Self::RuntimeVerify => "runtime_verify",
            Self::S3 => "s3",
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
    const fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Migrate => "migrate",
            Self::Query => "query",
            Self::Checkpoint => "checkpoint",
        }
    }
}

/// S3 operation label.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum S3Op {
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

impl S3Op {
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

/// S3 result label.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum S3Result {
    /// Success.
    Success,
    /// Failure.
    Failure,
}

impl S3Result {
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
    s3_total: [u64; 10],
    s3_duration: [f64; 5],
    cache_bytes: u64,
    cache_entries: u64,
    cache_hits: u64,
    integrity_errors: u64,
    last_supervisor: Option<SupervisorState>,
    last_attempt: Option<u32>,
    runtime_start: Option<Instant>,
}

/// Fixed-series metrics registry.
#[derive(Debug)]
pub struct MetricsRegistry {
    max_label: u64,
    inner: Mutex<Inner>,
}

impl MetricsRegistry {
    /// Construct after validating configured bounds.
    pub fn new(
        config: &MetricsConfig,
        version: &str,
        workerd_version: &str,
    ) -> Result<Self, PlatformError> {
        Self::validate_limits(config)?;
        if version.len() as u64 > config.max_label_value_bytes
            || workerd_version.len() as u64 > config.max_label_value_bytes
        {
            return Err(PlatformError::new(
                ErrorCode::LimitInvalid,
                "metrics label value exceeds configured max_label_value_bytes",
            ));
        }
        Ok(Self {
            max_label: config.max_label_value_bytes,
            inner: Mutex::new(Inner {
                version: version.to_owned(),
                workerd_version: workerd_version.to_owned(),
                start_total: [0; 16],
                restart_total: [0; 4],
                process_up: 0,
                start_duration: 0.0,
                sqlite_duration: [0.0; 4],
                s3_total: [0; 10],
                s3_duration: [0.0; 5],
                cache_bytes: 0,
                cache_entries: 0,
                cache_hits: 0,
                integrity_errors: 0,
                last_supervisor: None,
                last_attempt: None,
                runtime_start: None,
            }),
        })
    }

    /// Reject configured limits that cannot hold the required fixed set.
    pub fn validate_limits(config: &MetricsConfig) -> Result<(), PlatformError> {
        if config.max_series < REQUIRED_SERIES {
            return Err(PlatformError::new(
                ErrorCode::LimitInvalid,
                "metrics.max_series cannot contain the required fixed series set",
            ));
        }
        if config.max_label_value_bytes < MIN_LABEL_VALUE_BYTES {
            return Err(PlatformError::new(
                ErrorCode::LimitInvalid,
                "metrics.max_label_value_bytes cannot contain the required fixed labels",
            ));
        }
        Ok(())
    }

    /// Increment a start-stage counter.
    pub fn inc_start(&self, result: StartResult, stage: StartStage) {
        let i = start_index(result, stage);
        let mut g = self.lock();
        g.start_total[i] = g.start_total[i].saturating_add(1);
    }

    /// Increment a restart counter.
    pub fn inc_restart(&self, reason: RestartReason) {
        let i = restart_index(reason);
        let mut g = self.lock();
        g.restart_total[i] = g.restart_total[i].saturating_add(1);
    }

    /// Record workerd up (1) or down (0).
    pub fn set_process_up(&self, up: bool) {
        self.lock().process_up = u64::from(up);
    }

    /// Set the verified workerd version label. Must run before `/metrics` is exposed.
    pub fn set_workerd_version(&self, workerd_version: &str) -> Result<(), PlatformError> {
        if workerd_version.len() as u64 > self.max_label {
            return Err(PlatformError::new(
                ErrorCode::LimitInvalid,
                "metrics label value exceeds configured max_label_value_bytes",
            ));
        }
        self.lock().workerd_version = workerd_version.to_owned();
        Ok(())
    }

    /// Record last start duration in seconds from supervisor timing.
    pub fn observe_start_duration(&self, duration: Duration) {
        self.lock().start_duration = duration.as_secs_f64();
    }

    /// Record the five successful preflight operations. Does nothing for a failure.
    pub fn observe_preflight_success(&self, outcome: &open_compute_artifacts::PreflightOutcome) {
        let mut g = self.lock();
        for _ in 0..outcome.puts() {
            let i = s3_total_index(S3Op::Put, S3Result::Success);
            g.s3_total[i] = g.s3_total[i].saturating_add(1);
        }
        for _ in 0..outcome.heads() {
            let i = s3_total_index(S3Op::Head, S3Result::Success);
            g.s3_total[i] = g.s3_total[i].saturating_add(1);
        }
        for _ in 0..outcome.gets() {
            let i = s3_total_index(S3Op::Get, S3Result::Success);
            g.s3_total[i] = g.s3_total[i].saturating_add(1);
        }
        for _ in 0..outcome.deletes() {
            let i = s3_total_index(S3Op::Delete, S3Result::Success);
            g.s3_total[i] = g.s3_total[i].saturating_add(1);
        }
    }

    /// Current restart counter for tests and status snapshots.
    #[must_use]
    pub fn restart_total(&self, reason: RestartReason) -> u64 {
        self.lock().restart_total[restart_index(reason)]
    }

    /// Current S3 counter.
    #[must_use]
    pub fn s3_total(&self, op: S3Op, result: S3Result) -> u64 {
        self.lock().s3_total[s3_total_index(op, result)]
    }

    /// Record last sqlite op duration.
    pub fn observe_sqlite(&self, op: SqliteOp, duration: Duration) {
        self.lock().sqlite_duration[sqlite_index(op)] = duration.as_secs_f64();
    }

    /// Record an S3 request.
    pub fn observe_s3(&self, op: S3Op, result: S3Result, duration: Duration) {
        let mut g = self.lock();
        g.s3_total[s3_total_index(op, result)] =
            g.s3_total[s3_total_index(op, result)].saturating_add(1);
        g.s3_duration[s3_op_index(op)] = duration.as_secs_f64();
    }

    /// Apply a supervisor snapshot without double-counting coalesced repeats.
    pub fn observe_supervisor(&self, snap: &SupervisorSnapshot) {
        let mut g = self.lock();
        let state = snap.state;
        g.process_up = u64::from(state == SupervisorState::Running);
        if g.last_supervisor == Some(state) && g.last_attempt == Some(snap.attempt) {
            return;
        }
        let prev_state = g.last_supervisor;
        let prev_attempt = g.last_attempt;

        if state == SupervisorState::Starting {
            g.runtime_start = Some(Instant::now());
        }
        if state == SupervisorState::Running
            && let Some(start) = g.runtime_start.take()
        {
            g.start_duration = start.elapsed().as_secs_f64();
        }

        // Attempt 1 is the initial start. Attempt N represents N-1 logical restarts,
        // including coalesced jumps (1 -> 3) and a first observation already at N.
        let accounted = prev_attempt.unwrap_or(0).saturating_sub(1);
        let observed = snap.attempt.saturating_sub(1);
        let delta = observed.saturating_sub(accounted);
        if delta > 0 {
            let i = restart_index(RestartReason::UnexpectedExit);
            g.restart_total[i] = g.restart_total[i].saturating_add(u64::from(delta));
        } else if prev_state == Some(SupervisorState::Starting)
            && state == SupervisorState::Failed
            && snap.attempt <= 1
        {
            let i = restart_index(RestartReason::ProbeFailed);
            g.restart_total[i] = g.restart_total[i].saturating_add(1);
        }

        g.last_supervisor = Some(state);
        g.last_attempt = Some(snap.attempt);
    }

    /// Cache gauges.
    pub fn set_cache(&self, bytes: u64, entries: u64, hits: u64, integrity_errors: u64) {
        let mut g = self.lock();
        g.cache_bytes = bytes;
        g.cache_entries = entries;
        g.cache_hits = hits;
        g.integrity_errors = integrity_errors;
    }

    /// Prometheus text exposition, deterministically ordered.
    pub fn render(&self, status: &PlatformStatus) -> String {
        let g = self.lock();
        let mut out = String::new();
        write_help(
            &mut out,
            "platform_info",
            "gauge",
            "Platform build identity",
        );
        writeln!(
            &mut out,
            "platform_info{{version=\"{}\",workerd_version=\"{}\"}} 1",
            escape(&g.version),
            escape(&g.workerd_version)
        )
        .ok();
        write_help(
            &mut out,
            "platform_ready",
            "gauge",
            "1 if the component is healthy",
        );
        for name in component_order() {
            let ready = status
                .components
                .iter()
                .find(|c| c.name == name)
                .is_some_and(|c| c.state == ComponentState::Healthy);
            writeln!(
                &mut out,
                "platform_ready{{component=\"{}\"}} {}",
                name.as_str(),
                u64::from(ready)
            )
            .ok();
        }
        write_help(
            &mut out,
            "platform_start_total",
            "counter",
            "Startup stage outcomes",
        );
        for result in [StartResult::Failure, StartResult::Success] {
            for stage in start_stages() {
                let i = start_index(result, stage);
                writeln!(
                    &mut out,
                    "platform_start_total{{result=\"{}\",stage=\"{}\"}} {}",
                    result.as_str(),
                    stage.as_str(),
                    g.start_total[i]
                )
                .ok();
            }
        }
        write_help(
            &mut out,
            "workerd_process_up",
            "gauge",
            "1 if workerd is up",
        );
        writeln!(&mut out, "workerd_process_up {}", g.process_up).ok();
        write_help(
            &mut out,
            "workerd_restart_total",
            "counter",
            "workerd restart counts",
        );
        for reason in restart_reasons() {
            writeln!(
                &mut out,
                "workerd_restart_total{{reason=\"{}\"}} {}",
                reason.as_str(),
                g.restart_total[restart_index(reason)]
            )
            .ok();
        }
        write_help(
            &mut out,
            "workerd_start_duration_seconds",
            "gauge",
            "Last workerd start duration",
        );
        writeln!(
            &mut out,
            "workerd_start_duration_seconds {}",
            g.start_duration
        )
        .ok();
        write_help(
            &mut out,
            "sqlite_operation_duration_seconds",
            "gauge",
            "Last sqlite operation duration",
        );
        for op in sqlite_ops() {
            writeln!(
                &mut out,
                "sqlite_operation_duration_seconds{{database=\"control\",operation=\"{}\"}} {}",
                op.as_str(),
                g.sqlite_duration[sqlite_index(op)]
            )
            .ok();
        }
        write_help(&mut out, "s3_request_total", "counter", "S3 request counts");
        for op in s3_ops() {
            for result in [S3Result::Failure, S3Result::Success] {
                writeln!(
                    &mut out,
                    "s3_request_total{{operation=\"{}\",result=\"{}\"}} {}",
                    op.as_str(),
                    result.as_str(),
                    g.s3_total[s3_total_index(op, result)]
                )
                .ok();
            }
        }
        write_help(
            &mut out,
            "s3_request_duration_seconds",
            "gauge",
            "Last S3 request duration",
        );
        for op in s3_ops() {
            writeln!(
                &mut out,
                "s3_request_duration_seconds{{operation=\"{}\"}} {}",
                op.as_str(),
                g.s3_duration[s3_op_index(op)]
            )
            .ok();
        }
        write_help(
            &mut out,
            "artifact_cache_bytes",
            "gauge",
            "Cache byte total",
        );
        writeln!(&mut out, "artifact_cache_bytes {}", g.cache_bytes).ok();
        write_help(
            &mut out,
            "artifact_cache_entries",
            "gauge",
            "Cache entry total",
        );
        writeln!(&mut out, "artifact_cache_entries {}", g.cache_entries).ok();
        write_help(
            &mut out,
            "artifact_cache_hit_total",
            "counter",
            "Cache hit total",
        );
        writeln!(&mut out, "artifact_cache_hit_total {}", g.cache_hits).ok();
        write_help(
            &mut out,
            "artifact_integrity_error_total",
            "counter",
            "Integrity error total",
        );
        writeln!(
            &mut out,
            "artifact_integrity_error_total {}",
            g.integrity_errors
        )
        .ok();
        let _ = self.max_label;
        out
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
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

fn component_order() -> [ComponentName; 7] {
    [
        ComponentName::Cache,
        ComponentName::ControlDb,
        ComponentName::DataDir,
        ComponentName::MasterKey,
        ComponentName::Process,
        ComponentName::Runtime,
        ComponentName::S3,
    ]
}

fn start_stages() -> [StartStage; 8] {
    [
        StartStage::Cache,
        StartStage::Compile,
        StartStage::Config,
        StartStage::Listen,
        StartStage::RuntimeVerify,
        StartStage::S3,
        StartStage::Storage,
        StartStage::Supervisor,
    ]
}

fn restart_reasons() -> [RestartReason; 4] {
    [
        RestartReason::BudgetExhausted,
        RestartReason::ProbeFailed,
        RestartReason::UnexpectedExit,
        RestartReason::Unhealthy,
    ]
}

fn sqlite_ops() -> [SqliteOp; 4] {
    [
        SqliteOp::Checkpoint,
        SqliteOp::Migrate,
        SqliteOp::Open,
        SqliteOp::Query,
    ]
}

fn s3_ops() -> [S3Op; 5] {
    [S3Op::Delete, S3Op::Get, S3Op::Head, S3Op::List, S3Op::Put]
}

fn start_index(result: StartResult, stage: StartStage) -> usize {
    let r = match result {
        StartResult::Failure => 0,
        StartResult::Success => 1,
    };
    let s = start_stages().iter().position(|x| *x == stage).unwrap();
    r * 8 + s
}

fn restart_index(reason: RestartReason) -> usize {
    restart_reasons().iter().position(|x| *x == reason).unwrap()
}

fn sqlite_index(op: SqliteOp) -> usize {
    sqlite_ops().iter().position(|x| *x == op).unwrap()
}

fn s3_op_index(op: S3Op) -> usize {
    s3_ops().iter().position(|x| *x == op).unwrap()
}

fn s3_total_index(op: S3Op, result: S3Result) -> usize {
    let r = match result {
        S3Result::Failure => 0,
        S3Result::Success => 1,
    };
    s3_op_index(op) * 2 + r
}

/// Prometheus content type.
pub const CONTENT_TYPE: &str = "text/plain; version=0.0.4; charset=utf-8";
