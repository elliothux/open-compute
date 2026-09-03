//! Authorized private data plane for the loaded-isolate D1 facade.

use crate::d1_coordinator::D1Coordinator;
#[cfg(test)]
use crate::d1_coordinator::D1HandleManager;
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
    AccountId, BindingId, BindingKind, D1Config, ErrorCode, OperationClass, PlatformError,
    ResourceId, VersionId,
};
use open_compute_storage::{
    BindingRepository, D1Engine, D1Migration, D1MigrationRecord, D1QueryLimits, D1Statement,
    D1StatementResult, D1Value, PlatformStorage,
};
use open_compute_workers::ResourcePins;
use std::str::FromStr;
use std::sync::Arc;
#[cfg(any(test, feature = "test-support"))]
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

const ERROR_HEADER: &str = "x-open-compute-error-code";

/// Fully composed D1 executor with per-database serialized lanes.
#[derive(Clone)]
pub struct D1BindingService {
    storage: Arc<PlatformStorage>,
    config: D1Config,
    coordinator: Arc<D1Coordinator>,
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
            coordinator: Arc::new(D1Coordinator::new(storage.clone(), pins, config.clone())),
            storage,
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
        self.coordinator.set_metrics(metrics.clone());
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

    /// List user-visible tables for the operator dashboard.
    pub async fn operator_list_tables(
        &self,
        account_id: AccountId,
        resource_id: ResourceId,
    ) -> Result<Vec<String>, PlatformError> {
        self.run_control(account_id, resource_id, false, |engine, limits| {
            let statement = D1Statement {
                sql: "SELECT name FROM sqlite_master WHERE type = 'table' \
                      AND name NOT LIKE 'sqlite_%' \
                      AND name NOT LIKE '__open_compute_%' \
                      ORDER BY name"
                    .to_owned(),
                params: vec![],
            };
            let result = engine.query(&statement, limits)?;
            Ok(result
                .rows
                .into_iter()
                .filter_map(|row| match row.into_iter().next()? {
                    D1Value::Text(name) => Some(name),
                    _ => None,
                })
                .collect())
        })
        .await
    }

    /// Execute one bounded SQL statement for the operator dashboard.
    pub async fn operator_query(
        &self,
        account_id: AccountId,
        resource_id: ResourceId,
        sql: String,
    ) -> Result<D1StatementResult, PlatformError> {
        if sql.trim().is_empty() {
            return Err(PlatformError::new(
                ErrorCode::ConfigInvalid,
                "D1 SQL must not be empty",
            ));
        }
        let timeout = Duration::from_millis(self.config.batch_timeout_ms);
        let metrics = self.metrics.clone();
        self.coordinator
            .execute(account_id, resource_id, timeout, false, move |context| {
                let limits = D1QueryLimits::batch(context.config)?;
                let statement = D1Statement {
                    sql,
                    params: vec![],
                };
                let readonly = context
                    .engine
                    .statements_readonly(std::slice::from_ref(&statement), limits)?;
                let _admission = if readonly {
                    None
                } else {
                    context.mark_mutation();
                    ensure_d1_storage_headroom(context.storage)?;
                    let result = context.storage.reserve_mutation(64 * 1024);
                    if let Some(metrics) = &metrics {
                        metrics.observe_admission(
                            OperationClass::D1,
                            result.as_ref().err().map(PlatformError::code),
                        );
                    }
                    Some(result?)
                };
                context.engine.query(&statement, limits)
            })
            .await
    }

    /// Execute one official D1 query or one atomic batch through the shared database lane.
    pub(crate) async fn cloudflare_v4_query(
        &self,
        account_id: AccountId,
        resource_id: ResourceId,
        statements: Vec<D1Statement>,
    ) -> Result<Vec<D1StatementResult>, PlatformError> {
        if statements.is_empty() {
            return Err(PlatformError::new(
                ErrorCode::ConfigInvalid,
                "D1 query batch must not be empty",
            ));
        }
        self.run_control(account_id, resource_id, true, move |engine, limits| {
            if statements.len() == 1 {
                return engine
                    .query(&statements[0], limits)
                    .map(|result| vec![result]);
            }
            engine.batch(&statements, limits)
        })
        .await
    }

    /// Issue the current persisted database session bookmark for official time travel reads.
    pub(crate) async fn cloudflare_v4_bookmark(
        &self,
        account_id: AccountId,
        resource_id: ResourceId,
    ) -> Result<String, PlatformError> {
        let storage = self.storage.clone();
        self.run_control(account_id, resource_id, false, move |engine, _| {
            let version = engine.session_version()?;
            storage
                .crypto()
                .seal_d1_bookmark(account_id, resource_id, version)
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
        let metrics = self.metrics.clone();
        self.coordinator
            .execute(account_id, resource_id, timeout, mutation, move |context| {
                let engine = context.engine;
                let storage = context.storage;
                let config = context.config;
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
                    context.mark_mutation();
                }
                let result = operation(&engine, D1QueryLimits::batch(&config)?)?;
                Ok(result)
            })
            .await
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
        let version = parse_header::<VersionId>(headers, "x-open-compute-version-id")?;
        let descriptor = parse_digest(headers)?;
        parse_request_id(headers)?;
        if !content_type_is(headers, D1_FRAME_CONTENT_TYPE) {
            return Err(protocol_error());
        }
        let binding = BindingRepository::new(self.storage.db()).authorize(
            binding_id,
            version,
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
        let metrics = self.metrics.clone();
        let account_id = binding.account_id;
        let resource_id = binding.resource.id;
        let mutation_possible = matches!(command, Command::Exec(_));
        let result = self
            .coordinator
            .execute(
                account_id,
                resource_id,
                timeout,
                mutation_possible,
                move |context| {
                    let engine = context.engine;
                    let storage = context.storage;
                    let config = context.config;
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
                                context.mark_mutation();
                            }
                            apply_session(
                                storage.crypto(),
                                binding.account_id,
                                binding.resource.id,
                                engine,
                                &query.session,
                            )?;
                            let results = if query.mode == D1QueryMode::Batch {
                                engine.batch(&query.statements, limits)?
                            } else {
                                vec![engine.query(&query.statements[0], limits)?]
                            };
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
                                engine,
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
                            context.mark_mutation();
                            let result = engine.exec(&sql, D1QueryLimits::query(&config)?)?;
                            serde_json::to_vec(&result)
                                .map(|bytes| CommandResult::Json { bytes })
                                .map_err(|_| protocol_error())
                        }
                    }
                },
            )
            .await?;
        #[cfg(any(test, feature = "test-support"))]
        let mutation_completed = match &result {
            CommandResult::Frame { readonly, .. } => !readonly,
            CommandResult::Json { .. } => true,
        };
        #[cfg(any(test, feature = "test-support"))]
        if mutation_completed && self.response_loss_once.swap(false, Ordering::AcqRel) {
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

#[cfg(test)]
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

#[cfg(test)]
fn wall_now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| {
            i64::try_from(duration.as_millis()).unwrap_or(i64::MAX)
        })
}

#[cfg(test)]
#[path = "d1_backend_tests.rs"]
mod tests;
