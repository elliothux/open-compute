//! Authorized private data plane for the loaded-isolate D1 facade.

use crate::d1_protocol::{
    D1_FRAME_CONTENT_TYPE, D1_JSON_CONTENT_TYPE, D1_MAX_FRAME_BYTES, D1QueryMode, decode_exec,
    decode_query, encode_results,
};
use crate::d1_session::{apply_session, issue_bookmark};
use crate::metrics::{D1Operation as D1MetricOperation, MetricsRegistry};
use axum::body::{Body, to_bytes};
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use open_compute_core::{
    AccountId, BindingId, BindingKind, D1Config, DeploymentId, ErrorCode, OperationClass,
    PlatformError, ResourceAvailability, ResourceId,
};
use open_compute_storage::{
    BindingRepository, D1DatabaseRepository, D1Engine, D1Migration, D1MigrationRecord, D1Paths,
    D1QueryLimits, PlatformStorage, ResourceRepository,
};
use open_compute_workers::{ResourcePin, ResourcePins};
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const ERROR_HEADER: &str = "x-open-compute-error-code";

/// Fully composed D1 executor with per-database serialized lanes.
#[derive(Clone)]
pub struct D1BindingService {
    storage: Arc<PlatformStorage>,
    pins: ResourcePins,
    config: D1Config,
    handles: D1HandleManager,
    metrics: Option<Arc<MetricsRegistry>>,
    #[cfg(any(test, feature = "test-support"))]
    response_loss_once: Arc<AtomicBool>,
}

impl std::fmt::Debug for D1BindingService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("D1BindingService")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl D1BindingService {
    /// Bind persisted authority, resource pins, and validated D1 limits.
    #[must_use]
    pub fn new(storage: Arc<PlatformStorage>, pins: ResourcePins, config: D1Config) -> Self {
        Self {
            storage,
            pins,
            handles: D1HandleManager::new(
                config.max_open_databases,
                config.max_queued_operations_per_database,
                Duration::from_millis(config.idle_handle_ttl_ms),
            ),
            config,
            metrics: None,
            #[cfg(any(test, feature = "test-support"))]
            response_loss_once: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Inject one post-commit response loss for black-box result-unknown tests.
    #[cfg(any(test, feature = "test-support"))]
    #[must_use]
    pub fn with_response_loss_once(self) -> Self {
        self.arm_response_loss_once();
        self
    }

    /// Arm one post-commit response loss for a later black-box operation.
    #[cfg(any(test, feature = "test-support"))]
    pub fn arm_response_loss_once(&self) {
        self.response_loss_once.store(true, Ordering::Release);
    }

    /// Attach the process fixed-series metrics registry.
    #[must_use]
    pub fn with_metrics(mut self, metrics: Arc<MetricsRegistry>) -> Self {
        self.handles.set_metrics(metrics.clone());
        self.metrics = Some(metrics);
        self
    }

    /// Handle one generation-authenticated route from the shared backend.
    pub async fn handle(&self, request: axum::extract::Request) -> Response {
        let operation = metric_operation(request.uri().path());
        let started = Instant::now();
        match self.handle_inner(request).await {
            Ok(executed) => {
                if let Some(metrics) = &self.metrics {
                    metrics.observe_d1_operation(
                        executed.operation,
                        executed.readonly,
                        true,
                        started.elapsed(),
                        executed.rows_output,
                        executed.rows_written,
                        executed.result_bytes,
                    );
                }
                executed.response
            }
            Err(error) => {
                if let Some(metrics) = &self.metrics {
                    metrics.observe_product_error(OperationClass::D1, error.code());
                }
                if let (Some(metrics), Some(operation)) = (&self.metrics, operation) {
                    metrics.observe_d1_operation(
                        operation,
                        false,
                        false,
                        started.elapsed(),
                        0,
                        0,
                        0,
                    );
                    metrics.inc_d1_error(operation, error.code());
                }
                error_response(&error)
            }
        }
    }

    /// List one database's migration ledger through its serialized operation lane.
    pub async fn migrations(
        &self,
        account_id: AccountId,
        resource_id: ResourceId,
    ) -> Result<Vec<D1MigrationRecord>, PlatformError> {
        self.run_control(account_id, resource_id, false, |engine, _| {
            engine.migrations()
        })
        .await
    }

    /// Apply ordered migrations through the same lane used by tenant queries.
    pub async fn apply_migrations(
        &self,
        account_id: AccountId,
        resource_id: ResourceId,
        migrations: Vec<D1Migration>,
        now_ms: i64,
    ) -> Result<Vec<D1MigrationRecord>, PlatformError> {
        self.run_control(account_id, resource_id, true, move |engine, limits| {
            engine.apply_migrations(&migrations, limits, now_ms)
        })
        .await
    }

    /// Create a consistent local backup through the serialized database lane.
    pub async fn online_backup(
        &self,
        account_id: AccountId,
        resource_id: ResourceId,
        destination: std::path::PathBuf,
    ) -> Result<u32, PlatformError> {
        self.run_control(account_id, resource_id, false, move |engine, _| {
            engine.online_backup(&destination)?;
            engine.user_version()
        })
        .await
    }

    /// Read the tenant `user_version` through the serialized database lane.
    pub async fn user_version(
        &self,
        account_id: AccountId,
        resource_id: ResourceId,
    ) -> Result<u32, PlatformError> {
        self.run_control(account_id, resource_id, false, |engine, _| {
            engine.user_version()
        })
        .await
    }

    async fn run_control<T, F>(
        &self,
        account_id: AccountId,
        resource_id: ResourceId,
        mutation: bool,
        operation: F,
    ) -> Result<T, PlatformError>
    where
        T: Send + 'static,
        F: FnOnce(&D1Engine, D1QueryLimits) -> Result<T, PlatformError> + Send + 'static,
    {
        let timeout = Duration::from_millis(self.config.batch_timeout_ms);
        let pin = self.pins.try_pin(resource_id)?;
        let lane = self.handles.acquire(resource_id, timeout).await?;
        let storage = self.storage.clone();
        let config = self.config.clone();
        let metrics = self.metrics.clone();
        let task = tokio::task::spawn_blocking(move || {
            let _pin = pin;
            let _lane = lane;
            let catalog = D1DatabaseRepository::new(storage.db()).get(account_id, resource_id)?;
            if catalog.resource.availability != ResourceAvailability::Healthy {
                return Err(PlatformError::new(
                    ErrorCode::ResourceUnavailable,
                    "D1 database is quarantined",
                ));
            }
            let result = (|| {
                let paths = D1Paths::open(storage.data_dir().root())?;
                let path =
                    paths.resolve_storage_key(&catalog.storage_key, account_id, resource_id)?;
                let engine = D1Engine::from_record(path, &catalog)?;
                let _admission = if mutation {
                    let result =
                        storage.reserve_mutation(config.max_result_bytes.saturating_add(64 * 1024));
                    if let Some(metrics) = &metrics {
                        metrics.observe_admission(
                            OperationClass::D1,
                            result.as_ref().err().map(PlatformError::code),
                        );
                    }
                    Some(result?)
                } else {
                    None
                };
                if mutation {
                    ensure_d1_storage_headroom(&storage)?;
                }
                let result = operation(&engine, D1QueryLimits::batch(&config)?)?;
                if mutation {
                    engine.checkpoint(false)?;
                }
                if let Some(metrics) = &metrics
                    && let Ok(bytes) = engine.wal_bytes()
                {
                    metrics.observe_d1_wal_bytes(bytes);
                }
                Ok(result)
            })();
            persist_d1_corruption(&storage, account_id, resource_id, &result);
            result
        });
        match tokio::time::timeout(timeout.saturating_add(Duration::from_secs(1)), task).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(protocol_error()),
            Err(_) if mutation => Err(PlatformError::new(
                ErrorCode::D1ResultUnknown,
                "D1 control mutation result is unknown after timeout",
            )),
            Err(_) => Err(PlatformError::new(
                ErrorCode::D1Timeout,
                "D1 control operation exceeded its wall deadline",
            )),
        }
    }

    async fn handle_inner(
        &self,
        request: axum::extract::Request,
    ) -> Result<ExecutedResponse, PlatformError> {
        if request.method() != Method::POST {
            return Err(protocol_error());
        }
        let (binding_id, operation) = parse_path(request.uri().path())?;
        let headers = request.headers();
        let deployment = parse_header::<DeploymentId>(headers, "x-open-compute-deployment-id")?;
        let descriptor = parse_digest(headers)?;
        parse_request_id(headers)?;
        if !content_type_is(headers, D1_FRAME_CONTENT_TYPE) {
            return Err(protocol_error());
        }
        let binding = BindingRepository::new(self.storage.db()).authorize(
            binding_id,
            deployment,
            &descriptor,
        )?;
        if binding.binding.kind != BindingKind::D1Database
            || binding.binding.capability_version != 1
        {
            return Err(PlatformError::new(
                ErrorCode::BindingCapabilityUnsupported,
                "binding is not a supported D1 capability",
            ));
        }
        if operation == Operation::Exec && !binding.binding.permissions.write {
            return Err(permission_denied());
        }
        let pin = self.pins.try_pin(binding.resource.id)?;
        let body = to_bytes(request.into_body(), D1_MAX_FRAME_BYTES)
            .await
            .map_err(|_| limit_error())?;
        let command = match operation {
            Operation::Query => Command::Query(decode_query(&body)?),
            Operation::Exec => Command::Exec(decode_exec(&body)?),
        };
        let timeout = match &command {
            Command::Query(query) if query.mode == D1QueryMode::Batch => {
                Duration::from_millis(self.config.batch_timeout_ms)
            }
            _ => Duration::from_millis(self.config.query_timeout_ms),
        };
        let lane = self.handles.acquire(binding.resource.id, timeout).await?;
        let storage = self.storage.clone();
        let config = self.config.clone();
        let metrics = self.metrics.clone();
        let mutation_started = Arc::new(AtomicBool::new(false));
        let mutation_for_task = mutation_started.clone();
        let task = tokio::task::spawn_blocking(move || {
            let _pin: ResourcePin = pin;
            let _lane = lane;
            let catalog = D1DatabaseRepository::new(storage.db())
                .get(binding.account_id, binding.resource.id)?;
            if catalog.resource.availability != ResourceAvailability::Healthy {
                return Err(PlatformError::new(
                    ErrorCode::ResourceUnavailable,
                    "D1 database is quarantined",
                ));
            }
            let result = (|| {
                let paths = D1Paths::open(storage.data_dir().root())?;
                let path = paths.resolve_storage_key(
                    &catalog.storage_key,
                    binding.account_id,
                    binding.resource.id,
                )?;
                let engine = D1Engine::from_record(path, &catalog)?;
                match command {
                    Command::Query(query) => {
                        let limits = if query.mode == D1QueryMode::Batch {
                            D1QueryLimits::batch(&config)?
                        } else {
                            D1QueryLimits::query(&config)?
                        };
                        let readonly = engine.statements_readonly(&query.statements, limits)?;
                        if readonly && !binding.binding.permissions.read
                            || !readonly && !binding.binding.permissions.write
                        {
                            return Err(permission_denied());
                        }
                        let _admission = if readonly {
                            None
                        } else {
                            let result = storage.reserve_mutation(
                                config
                                    .max_result_bytes
                                    .saturating_add(D1_MAX_FRAME_BYTES as u64),
                            );
                            if let Some(metrics) = &metrics {
                                metrics.observe_admission(
                                    OperationClass::D1,
                                    result.as_ref().err().map(PlatformError::code),
                                );
                            }
                            Some(result?)
                        };
                        if !readonly {
                            ensure_d1_storage_headroom(&storage)?;
                        }
                        apply_session(
                            storage.crypto(),
                            binding.account_id,
                            binding.resource.id,
                            &engine,
                            &query.session,
                        )?;
                        mutation_for_task.store(!readonly, Ordering::Release);
                        let results = if query.mode == D1QueryMode::Batch {
                            engine.batch(&query.statements, limits)?
                        } else {
                            vec![engine.query(&query.statements[0], limits)?]
                        };
                        if !readonly {
                            engine.checkpoint(false)?;
                        }
                        if let Some(metrics) = &metrics
                            && let Ok(bytes) = engine.wal_bytes()
                        {
                            metrics.observe_d1_wal_bytes(bytes);
                        }
                        let rows_output = results
                            .iter()
                            .map(|result| result.meta.rows_read)
                            .fold(0_u64, u64::saturating_add);
                        let rows_written = results
                            .iter()
                            .map(|result| result.meta.rows_written)
                            .fold(0_u64, u64::saturating_add);
                        let (bookmark, session_version) = issue_bookmark(
                            storage.crypto(),
                            binding.account_id,
                            binding.resource.id,
                            &engine,
                            &query.session,
                        )?;
                        encode_results(&results, bookmark.as_deref(), session_version).map(
                            |bytes| CommandResult::Frame {
                                bytes,
                                operation: if query.mode == D1QueryMode::Batch {
                                    D1MetricOperation::Batch
                                } else {
                                    D1MetricOperation::Query
                                },
                                readonly,
                                rows_output,
                                rows_written,
                            },
                        )
                    }
                    Command::Exec(sql) => {
                        let result = storage.reserve_mutation(
                            config.max_result_bytes.saturating_add(sql.len() as u64),
                        );
                        if let Some(metrics) = &metrics {
                            metrics.observe_admission(
                                OperationClass::D1,
                                result.as_ref().err().map(PlatformError::code),
                            );
                        }
                        let _admission = result?;
                        ensure_d1_storage_headroom(&storage)?;
                        mutation_for_task.store(true, Ordering::Release);
                        let result = engine.exec(&sql, D1QueryLimits::query(&config)?)?;
                        engine.checkpoint(false)?;
                        if let Some(metrics) = &metrics
                            && let Ok(bytes) = engine.wal_bytes()
                        {
                            metrics.observe_d1_wal_bytes(bytes);
                        }
                        serde_json::to_vec(&result)
                            .map(|bytes| CommandResult::Json { bytes })
                            .map_err(|_| protocol_error())
                    }
                }
            })();
            persist_d1_corruption(&storage, binding.account_id, binding.resource.id, &result);
            result
        });
        let outer = timeout.saturating_add(Duration::from_secs(1));
        let result = match tokio::time::timeout(outer, task).await {
            Ok(Ok(result)) => result?,
            Ok(Err(_)) => return Err(protocol_error()),
            Err(_) if mutation_started.load(Ordering::Acquire) => {
                return Err(PlatformError::new(
                    ErrorCode::D1ResultUnknown,
                    "D1 mutation result is unknown after transport timeout",
                ));
            }
            Err(_) => {
                return Err(PlatformError::new(
                    ErrorCode::D1Timeout,
                    "D1 query exceeded its wall deadline",
                ));
            }
        };
        #[cfg(any(test, feature = "test-support"))]
        if mutation_started.load(Ordering::Acquire)
            && self.response_loss_once.swap(false, Ordering::AcqRel)
        {
            return Err(PlatformError::new(
                ErrorCode::D1ResultUnknown,
                "D1 mutation committed but its response was lost",
            ));
        }
        Ok(match result {
            CommandResult::Frame {
                bytes,
                operation: metric_operation,
                readonly,
                rows_output,
                rows_written,
            } => ExecutedResponse {
                result_bytes: bytes.len() as u64,
                response: response(bytes, D1_FRAME_CONTENT_TYPE),
                operation: metric_operation,
                readonly,
                rows_output,
                rows_written,
            },
            CommandResult::Json { bytes } => ExecutedResponse {
                result_bytes: bytes.len() as u64,
                response: response(bytes, D1_JSON_CONTENT_TYPE),
                operation: D1MetricOperation::Exec,
                readonly: false,
                rows_output: 0,
                rows_written: 0,
            },
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Operation {
    Query,
    Exec,
}

enum Command {
    Query(crate::d1_protocol::D1QueryRequest),
    Exec(String),
}

enum CommandResult {
    Frame {
        bytes: Vec<u8>,
        operation: D1MetricOperation,
        readonly: bool,
        rows_output: u64,
        rows_written: u64,
    },
    Json {
        bytes: Vec<u8>,
    },
}

struct ExecutedResponse {
    response: Response,
    operation: D1MetricOperation,
    readonly: bool,
    rows_output: u64,
    rows_written: u64,
    result_bytes: u64,
}

#[derive(Clone)]
struct D1HandleManager {
    max_open: usize,
    queue_limit: usize,
    idle_ttl: Duration,
    lanes: Arc<Mutex<HashMap<ResourceId, Arc<D1Lane>>>>,
    metrics: Arc<Mutex<Option<Arc<MetricsRegistry>>>>,
}

impl D1HandleManager {
    fn new(global: u32, queue_limit: u32, idle_ttl: Duration) -> Self {
        Self {
            max_open: global.max(1) as usize,
            queue_limit: queue_limit.max(1) as usize,
            idle_ttl,
            lanes: Arc::new(Mutex::new(HashMap::new())),
            metrics: Arc::new(Mutex::new(None)),
        }
    }

    fn set_metrics(&self, metrics: Arc<MetricsRegistry>) {
        *self
            .metrics
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(metrics);
    }

    async fn acquire(
        &self,
        resource: ResourceId,
        timeout: Duration,
    ) -> Result<D1LaneLease, PlatformError> {
        let (lane, open_databases) = {
            let mut lanes = self
                .lanes
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(lane) = lanes.get(&resource) {
                (lane.clone(), lanes.len())
            } else {
                let now = Instant::now();
                lanes.retain(|_, lane| {
                    lane.queued.load(Ordering::Acquire) > 0
                        || lane.semaphore.available_permits() == 0
                        || now.duration_since(lane.last_used()) < self.idle_ttl
                });
                if lanes.len() >= self.max_open {
                    let candidate = lanes
                        .iter()
                        .filter(|(_, lane)| {
                            lane.queued.load(Ordering::Acquire) == 0
                                && lane.semaphore.available_permits() == 1
                        })
                        .min_by_key(|(_, lane)| lane.last_used())
                        .map(|(id, _)| *id);
                    let Some(candidate) = candidate else {
                        return Err(overloaded());
                    };
                    lanes.remove(&candidate);
                }
                let lane = Arc::new(D1Lane {
                    semaphore: Arc::new(tokio::sync::Semaphore::new(1)),
                    queued: AtomicUsize::new(0),
                    last_used: Mutex::new(now),
                });
                lanes.insert(resource, lane.clone());
                (lane, lanes.len())
            }
        };
        let prior = lane.queued.fetch_add(1, Ordering::AcqRel);
        if let Some(metrics) = self
            .metrics
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
        {
            metrics.set_d1_open_databases(open_databases as u64);
            metrics.observe_d1_queue_depth(prior.saturating_add(1) as u64);
        }
        if prior >= self.queue_limit {
            lane.queued.fetch_sub(1, Ordering::AcqRel);
            return Err(overloaded());
        }
        let resource_permit = tokio::time::timeout(timeout, lane.semaphore.clone().acquire_owned())
            .await
            .map_err(|_| overloaded())?
            .map_err(|_| overloaded());
        lane.queued.fetch_sub(1, Ordering::AcqRel);
        let resource_permit = resource_permit?;
        Ok(D1LaneLease {
            _resource: resource_permit,
            _lane: lane,
        })
    }
}

struct D1Lane {
    semaphore: Arc<tokio::sync::Semaphore>,
    queued: AtomicUsize,
    last_used: Mutex<Instant>,
}

impl D1Lane {
    fn last_used(&self) -> Instant {
        *self
            .last_used
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

struct D1LaneLease {
    _resource: tokio::sync::OwnedSemaphorePermit,
    _lane: Arc<D1Lane>,
}

impl Drop for D1LaneLease {
    fn drop(&mut self) {
        *self
            ._lane
            .last_used
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Instant::now();
    }
}

fn parse_path(path: &str) -> Result<(BindingId, Operation), PlatformError> {
    let rest = path
        .strip_prefix("/internal/bindings/v1/d1/")
        .ok_or_else(protocol_error)?;
    let (id, operation) = rest.split_once('/').ok_or_else(protocol_error)?;
    if operation.contains('/') {
        return Err(protocol_error());
    }
    let operation = match operation {
        "query" => Operation::Query,
        "exec" => Operation::Exec,
        _ => return Err(protocol_error()),
    };
    Ok((
        BindingId::from_str(id).map_err(|_| protocol_error())?,
        operation,
    ))
}

fn metric_operation(path: &str) -> Option<D1MetricOperation> {
    if path.ends_with("/exec") {
        Some(D1MetricOperation::Exec)
    } else if path.ends_with("/query") {
        Some(D1MetricOperation::Query)
    } else {
        None
    }
}

fn parse_header<T: FromStr>(headers: &HeaderMap, name: &str) -> Result<T, PlatformError> {
    header_text(headers, name)
        .and_then(|value| T::from_str(value).ok())
        .ok_or_else(protocol_error)
}

fn parse_digest(headers: &HeaderMap) -> Result<[u8; 32], PlatformError> {
    let value =
        header_text(headers, "x-open-compute-descriptor-sha256").ok_or_else(protocol_error)?;
    hex::decode(value)
        .map_err(|_| protocol_error())?
        .try_into()
        .map_err(|_| protocol_error())
}

fn parse_request_id(headers: &HeaderMap) -> Result<(), PlatformError> {
    let value = header_text(headers, "x-open-compute-request-id").ok_or_else(protocol_error)?;
    let parsed = uuid::Uuid::parse_str(value).map_err(|_| protocol_error())?;
    if parsed.hyphenated().to_string() != value {
        return Err(protocol_error());
    }
    Ok(())
}

fn header_text<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name)?.to_str().ok()
}

fn content_type_is(headers: &HeaderMap, expected: &str) -> bool {
    header_text(headers, header::CONTENT_TYPE.as_str())
        .and_then(|value| value.split(';').next())
        .is_some_and(|value| value.trim() == expected)
}

fn response(bytes: Vec<u8>, content_type: &'static str) -> Response {
    let length = bytes.len();
    let mut response = Response::new(Body::from(bytes));
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    if let Ok(value) = HeaderValue::from_str(&length.to_string()) {
        response.headers_mut().insert(header::CONTENT_LENGTH, value);
    }
    response
}

fn error_response(error: &PlatformError) -> Response {
    let status = match error.code() {
        ErrorCode::BindingNotFound | ErrorCode::ResourceNotFound => StatusCode::NOT_FOUND,
        ErrorCode::BindingPermissionDenied => StatusCode::FORBIDDEN,
        ErrorCode::D1LimitError | ErrorCode::BindingLimitExceeded => StatusCode::PAYLOAD_TOO_LARGE,
        ErrorCode::D1Overloaded | ErrorCode::D1Timeout => StatusCode::TOO_MANY_REQUESTS,
        ErrorCode::D1ResultUnknown | ErrorCode::ResourceUnavailable => {
            StatusCode::SERVICE_UNAVAILABLE
        }
        ErrorCode::D1DatabaseCorrupt | ErrorCode::D1IdentityMismatch => {
            StatusCode::UNPROCESSABLE_ENTITY
        }
        ErrorCode::BindingCapabilityUnsupported | ErrorCode::BindingTypeMismatch => {
            StatusCode::UNPROCESSABLE_ENTITY
        }
        ErrorCode::D1TypeError
        | ErrorCode::D1SqlInvalid
        | ErrorCode::D1ParameterMismatch
        | ErrorCode::D1AuthorizerDenied
        | ErrorCode::D1InvalidBatch
        | ErrorCode::D1SessionError
        | ErrorCode::D1DumpError
        | ErrorCode::D1InternalProtocolError => StatusCode::BAD_REQUEST,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    };
    let body = serde_json::json!({
        "ok": false,
        "error": {
            "code": error.code().as_str(),
            "retryable": matches!(error.code(), ErrorCode::D1Overloaded | ErrorCode::D1Timeout),
            "resultUnknown": error.code() == ErrorCode::D1ResultUnknown,
        }
    });
    let mut response = (status, axum::Json(body)).into_response();
    if let Ok(value) = HeaderValue::from_str(error.code().as_str()) {
        response
            .headers_mut()
            .insert(HeaderName::from_static(ERROR_HEADER), value);
    }
    response
}

fn protocol_error() -> PlatformError {
    PlatformError::new(
        ErrorCode::D1InternalProtocolError,
        "D1 private request is invalid",
    )
}

fn limit_error() -> PlatformError {
    PlatformError::new(
        ErrorCode::D1LimitError,
        "D1 private request exceeded its fixed limit",
    )
}

fn overloaded() -> PlatformError {
    PlatformError::new(ErrorCode::D1Overloaded, "D1 operation queue is saturated")
}

fn permission_denied() -> PlatformError {
    PlatformError::new(
        ErrorCode::BindingPermissionDenied,
        "binding permissions reject this D1 operation",
    )
}

pub(crate) fn ensure_d1_storage_headroom(storage: &PlatformStorage) -> Result<(), PlatformError> {
    let stat = rustix::fs::statvfs(storage.data_dir().root()).map_err(|_| {
        PlatformError::new(
            ErrorCode::ResourceUnavailable,
            "D1 filesystem capacity is unavailable",
        )
    })?;
    let available = stat.f_bavail.saturating_mul(stat.f_frsize);
    if available < storage.free_space_hard_bytes() {
        return Err(PlatformError::new(
            ErrorCode::D1DatabaseFull,
            "D1 filesystem free-space safety floor was reached",
        ));
    }
    Ok(())
}

fn wall_now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| {
            i64::try_from(duration.as_millis()).unwrap_or(i64::MAX)
        })
}

fn persist_d1_corruption<T>(
    storage: &PlatformStorage,
    account_id: AccountId,
    resource_id: ResourceId,
    result: &Result<T, PlatformError>,
) {
    let Err(error) = result else { return };
    let code = match error.code() {
        ErrorCode::D1DatabaseCorrupt => "D1_DATABASE_CORRUPT",
        ErrorCode::D1IdentityMismatch => "D1_IDENTITY_MISMATCH",
        _ => return,
    };
    let _ = ResourceRepository::new(storage.db()).set_availability(
        account_id,
        resource_id,
        ResourceAvailability::Unavailable,
        Some(code),
        wall_now_ms(),
    );
}

#[cfg(test)]
#[path = "d1_backend_tests.rs"]
mod tests;
