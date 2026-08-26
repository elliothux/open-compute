//! Authenticated Queue catalog and lifecycle control API.

use crate::http::{HttpState, ProductErrorCode, authorize};
use crate::metrics::{MetricsRegistry, QueueReconcileOperation};
#[path = "queue_reconcile.rs"]
mod queue_reconcile;
use axum::Router;
use axum::body::{Body, to_bytes};
use axum::extract::{Path, Request, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use open_compute_core::{AccountId, ErrorCode, PlatformError, QueueId, RequestId};
use open_compute_storage::{
    IdempotencyReservation, PlatformStorage, QUEUE_DEFAULT_MAX_BACKLOG_BYTES, QueueAvailability,
    QueueConfig, QueueRepository, QueueState, RunningQueueMutation, SchedulerStore,
    WorkerRepository,
};
use open_compute_workers::{CreateQueueOutcome, CreateQueueRequest, QueueController};
use queue_reconcile::{reconcile_running_mutations, resume_running_mutation};
use serde::{Deserialize, Serialize};
use std::str::FromStr;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const MAX_JSON_BODY: usize = 16 * 1024;
const IDEMPOTENCY_HEADER: &str = "idempotency-key";
const EXPECTED_LIFECYCLE_HEADER: &str = "x-open-compute-expected-lifecycle-generation";
const IDEMPOTENCY_TTL_MS: i64 = 24 * 60 * 60 * 1000;
const API_VERSION: &[u8] = b"open-compute/queue-control/v1\0";

/// Queue control-plane composition state.
#[derive(Clone, Debug)]
pub struct QueueApiState {
    storage: Arc<PlatformStorage>,
    scheduler: Arc<SchedulerStore>,
    metrics: Option<Arc<MetricsRegistry>>,
    default_max_backlog_bytes: u64,
}

impl QueueApiState {
    /// Bind the control catalog to its independent scheduler authority.
    #[must_use]
    pub const fn new(storage: Arc<PlatformStorage>, scheduler: Arc<SchedulerStore>) -> Self {
        Self {
            storage,
            scheduler,
            metrics: None,
            default_max_backlog_bytes: QUEUE_DEFAULT_MAX_BACKLOG_BYTES,
        }
    }

    /// Attach fixed low-cardinality reconciliation metrics.
    #[must_use]
    pub fn with_metrics(mut self, metrics: Arc<MetricsRegistry>) -> Self {
        self.metrics = Some(metrics);
        self
    }

    /// Set the operator default used when Queue create omits a backlog quota.
    #[must_use]
    pub fn with_default_max_backlog_bytes(mut self, bytes: u64) -> Self {
        self.default_max_backlog_bytes = bytes;
        self
    }

    /// Converge a bounded startup batch before producer traffic is exposed.
    pub async fn reconcile_pending(&self) -> Result<u32, PlatformError> {
        let storage = self.storage.clone();
        let scheduler = self.scheduler.clone();
        let now = now_ms();
        let (pending, result) = tokio::task::spawn_blocking(move || {
            let pending = QueueRepository::new(storage.db()).list_reconcile(256)?;
            let result =
                reconcile_running_mutations(&storage, &scheduler, 256).and_then(|mutations| {
                    QueueController::new(&storage, scheduler)
                        .reconcile_pending(256, now)
                        .map(|lifecycle| lifecycle.saturating_add(mutations))
                });
            Ok::<_, PlatformError>((pending, result))
        })
        .await
        .map_err(|_| internal())??;
        if let Some(metrics) = &self.metrics {
            let success = result.is_ok();
            for queue in pending {
                let operation = match queue.state {
                    QueueState::Creating => QueueReconcileOperation::Create,
                    QueueState::Ready => QueueReconcileOperation::Config,
                    QueueState::Deleting => QueueReconcileOperation::Delete,
                    QueueState::Tombstoned => continue,
                };
                let lag_ms = now.saturating_sub(queue.updated_at_ms).max(0);
                metrics.observe_queue_reconcile(
                    operation,
                    success,
                    Duration::from_millis(u64::try_from(lag_ms).unwrap_or(u64::MAX)),
                );
            }
        }
        result
    }
}

/// Queue management routes. Tenant message writes are intentionally absent.
pub fn control_router() -> Router<HttpState> {
    Router::new()
        .route(
            "/v1/accounts/{account_id}/queues",
            post(create_queue).get(list_queues),
        )
        .route(
            "/v1/accounts/{account_id}/queues/{queue_id}",
            get(get_queue).patch(patch_queue).delete(delete_queue),
        )
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CreateQueueBody {
    name: String,
    delivery_delay_seconds: Option<u32>,
    retention_seconds: Option<u32>,
    max_backlog_bytes: Option<u64>,
}

async fn create_queue(
    State(state): State<HttpState>,
    Path(account): Path<String>,
    request: Request,
) -> Response {
    let request_id = request_id(&request);
    let Some(api) = authorized_api(&state, &request) else {
        return unauthorized_or_unavailable(&state, &request, request_id);
    };
    let account_id = match parse_account(&account) {
        Ok(value) => value,
        Err(error) => return error_response(error, request_id),
    };
    let key = match idempotency_key(&request) {
        Ok(value) => value,
        Err(error) => return error_response(error, request_id),
    };
    let body = match read_json::<CreateQueueBody>(request).await {
        Ok(value) => value,
        Err(error) => return error_response(error, request_id),
    };
    let mut config = QueueConfig {
        max_backlog_bytes: api.default_max_backlog_bytes,
        ..QueueConfig::default()
    };
    if let Some(value) = body.delivery_delay_seconds {
        config.delivery_delay_seconds = value;
    }
    if let Some(value) = body.retention_seconds {
        config.retention_seconds = value;
    }
    if let Some(value) = body.max_backlog_bytes {
        config.max_backlog_bytes = value;
    }
    let storage = api.storage.clone();
    let scheduler = api.scheduler.clone();
    let result = tokio::task::spawn_blocking(move || {
        QueueController::new(&storage, scheduler).create(&CreateQueueRequest {
            account_id,
            name: body.name,
            config,
            idempotency_key: key,
            request_id,
            now_ms: now_ms(),
        })
    })
    .await;
    match result {
        Ok(Ok(CreateQueueOutcome::Applied(value))) => json_response(&value, StatusCode::CREATED),
        Ok(Ok(CreateQueueOutcome::Replay(bytes))) => json_bytes(bytes, StatusCode::OK),
        Ok(Err(error)) => error_response(error, request_id),
        Err(_) => error_response(internal(), request_id),
    }
}

async fn list_queues(
    State(state): State<HttpState>,
    Path(account): Path<String>,
    request: Request,
) -> Response {
    let request_id = request_id(&request);
    let Some(api) = authorized_api(&state, &request) else {
        return unauthorized_or_unavailable(&state, &request, request_id);
    };
    let account_id = match parse_account(&account) {
        Ok(value) => value,
        Err(error) => return error_response(error, request_id),
    };
    let (after, limit) = match parse_list_query(request.uri().query()) {
        Ok(value) => value,
        Err(error) => return error_response(error, request_id),
    };
    let storage = api.storage.clone();
    match tokio::task::spawn_blocking(move || {
        QueueRepository::new(storage.db()).list(account_id, after, limit)
    })
    .await
    {
        Ok(Ok(queues)) => {
            let next_cursor = (u32::try_from(queues.len()).ok() == Some(limit))
                .then(|| queues.last().map(queue_cursor))
                .flatten();
            json_response(
                &serde_json::json!({ "queues": queues, "nextCursor": next_cursor }),
                StatusCode::OK,
            )
        }
        Ok(Err(error)) => error_response(error, request_id),
        Err(_) => error_response(internal(), request_id),
    }
}

async fn get_queue(
    State(state): State<HttpState>,
    Path((account, queue)): Path<(String, String)>,
    request: Request,
) -> Response {
    let request_id = request_id(&request);
    let Some(api) = authorized_api(&state, &request) else {
        return unauthorized_or_unavailable(&state, &request, request_id);
    };
    let (account_id, queue_id) = match parse_ids(&account, &queue) {
        Ok(value) => value,
        Err(error) => return error_response(error, request_id),
    };
    let storage = api.storage.clone();
    let scheduler = api.scheduler.clone();
    match tokio::task::spawn_blocking(move || {
        let queue = QueueRepository::new(storage.db()).get(account_id, queue_id)?;
        let metrics = if queue.state == QueueState::Ready
            && queue.availability == QueueAvailability::Healthy
        {
            Some(scheduler.queue_metrics(
                queue.id,
                queue.lifecycle_generation,
                queue.config_generation,
            )?)
        } else {
            None
        };
        Ok::<_, PlatformError>((queue, metrics))
    })
    .await
    {
        Ok(Ok((queue, metrics))) => json_response(
            &serde_json::json!({ "queue": queue, "metrics": metrics }),
            StatusCode::OK,
        ),
        Ok(Err(error)) => error_response(error, request_id),
        Err(_) => error_response(internal(), request_id),
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PatchQueueBody {
    expected_config_generation: u64,
    name: Option<String>,
    delivery_delay_seconds: Option<u32>,
    retention_seconds: Option<u32>,
    max_backlog_bytes: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum QueueMutationIntent {
    Patch {
        version: u32,
        request_id: RequestId,
        body: PatchQueueBody,
    },
    Delete {
        version: u32,
        request_id: RequestId,
        expected_lifecycle_generation: u64,
        force: bool,
        purged_messages: Option<u64>,
        purged_bytes: Option<u64>,
    },
}

impl PatchQueueBody {
    fn changes_config(&self) -> bool {
        self.delivery_delay_seconds.is_some()
            || self.retention_seconds.is_some()
            || self.max_backlog_bytes.is_some()
    }
}

async fn patch_queue(
    State(state): State<HttpState>,
    Path((account, queue)): Path<(String, String)>,
    request: Request,
) -> Response {
    let request_id = request_id(&request);
    let Some(api) = authorized_api(&state, &request) else {
        return unauthorized_or_unavailable(&state, &request, request_id);
    };
    let (account_id, queue_id) = match parse_ids(&account, &queue) {
        Ok(value) => value,
        Err(error) => return error_response(error, request_id),
    };
    let key = match idempotency_key(&request) {
        Ok(value) => value,
        Err(error) => return error_response(error, request_id),
    };
    let body = match read_json::<PatchQueueBody>(request).await {
        Ok(value) => value,
        Err(error) => return error_response(error, request_id),
    };
    if body.expected_config_generation == 0 || body.name.is_some() == body.changes_config() {
        return error_response(
            PlatformError::new(
                ErrorCode::ConfigInvalid,
                "Queue PATCH must contain exactly one rename or config change",
            ),
            request_id,
        );
    }
    let canonical = serde_json::to_vec(&body).unwrap_or_default();
    let intent = QueueMutationIntent::Patch {
        version: 1,
        request_id,
        body,
    };
    let intent_json = serde_json::to_vec(&intent).unwrap_or_default();
    let storage = api.storage.clone();
    let scheduler = api.scheduler.clone();
    let result = tokio::task::spawn_blocking(move || {
        let scope = format!("queue.patch:{queue_id}");
        let fingerprint = mutation_fingerprint(&storage, b"patch", queue_id, &canonical);
        let mutation = RunningQueueMutation {
            account_id,
            scope: scope.clone(),
            idempotency_key: key.clone(),
            request_fingerprint: fingerprint,
            queue_id,
            intent_json: intent_json.clone(),
        };
        match reserve_mutation(&storage, &mutation)? {
            IdempotencyReservation::Complete(bytes) => return Ok(MutationOutcome::Replay(bytes)),
            IdempotencyReservation::Failed(bytes) => return Ok(MutationOutcome::Failed(bytes)),
            IdempotencyReservation::Running => return Err(idempotency_running()),
            IdempotencyReservation::Reserved => {}
        }
        match resume_running_mutation(&storage, scheduler, &mutation) {
            Ok(outcome) => Ok(outcome),
            Err(error) => {
                if is_final_mutation_failure(error.code()) {
                    persist_failure(
                        &storage,
                        account_id,
                        &scope,
                        &key,
                        &fingerprint,
                        error.code(),
                    )?;
                }
                Err(error)
            }
        }
    })
    .await;
    mutation_response(result, request_id, StatusCode::OK)
}

async fn delete_queue(
    State(state): State<HttpState>,
    Path((account, queue)): Path<(String, String)>,
    request: Request,
) -> Response {
    let request_id = request_id(&request);
    let Some(api) = authorized_api(&state, &request) else {
        return unauthorized_or_unavailable(&state, &request, request_id);
    };
    let (account_id, queue_id) = match parse_ids(&account, &queue) {
        Ok(value) => value,
        Err(error) => return error_response(error, request_id),
    };
    let key = match idempotency_key(&request) {
        Ok(value) => value,
        Err(error) => return error_response(error, request_id),
    };
    let expected_generation = match expected_lifecycle_generation(&request) {
        Ok(value) => value,
        Err(error) => return error_response(error, request_id),
    };
    let force = match parse_force(request.uri().query()) {
        Ok(value) => value,
        Err(error) => return error_response(error, request_id),
    };
    let storage = api.storage.clone();
    let scheduler = api.scheduler.clone();
    let result = tokio::task::spawn_blocking(move || {
        let scope = format!("queue.delete:{queue_id}");
        let canonical = [
            expected_generation.to_be_bytes().as_slice(),
            &[u8::from(force)],
        ]
        .concat();
        let fingerprint = mutation_fingerprint(&storage, b"delete", queue_id, &canonical);
        let intent = QueueMutationIntent::Delete {
            version: 1,
            request_id,
            expected_lifecycle_generation: expected_generation,
            force,
            purged_messages: None,
            purged_bytes: None,
        };
        let mutation = RunningQueueMutation {
            account_id,
            scope: scope.clone(),
            idempotency_key: key.clone(),
            request_fingerprint: fingerprint,
            queue_id,
            intent_json: serde_json::to_vec(&intent).map_err(|_| internal())?,
        };
        match reserve_mutation(&storage, &mutation)? {
            IdempotencyReservation::Complete(bytes) => return Ok(MutationOutcome::Replay(bytes)),
            IdempotencyReservation::Failed(bytes) => return Ok(MutationOutcome::Failed(bytes)),
            IdempotencyReservation::Running => return Err(idempotency_running()),
            IdempotencyReservation::Reserved => {}
        }
        match resume_running_mutation(&storage, scheduler, &mutation) {
            Ok(outcome) => Ok(outcome),
            Err(error) => {
                if is_final_mutation_failure(error.code()) {
                    persist_failure(
                        &storage,
                        account_id,
                        &scope,
                        &key,
                        &fingerprint,
                        error.code(),
                    )?;
                }
                Err(error)
            }
        }
    })
    .await;
    mutation_response(result, request_id, StatusCode::ACCEPTED)
}

enum MutationOutcome {
    Applied(Vec<u8>),
    Replay(Vec<u8>),
    Failed(Vec<u8>),
}

fn reserve_mutation(
    storage: &PlatformStorage,
    mutation: &RunningQueueMutation,
) -> Result<IdempotencyReservation, PlatformError> {
    QueueRepository::new(storage.db()).reserve_mutation(
        mutation.account_id,
        &mutation.scope,
        &mutation.idempotency_key,
        storage.crypto().fingerprint_key_id(),
        &mutation.request_fingerprint,
        mutation.queue_id,
        &mutation.intent_json,
        now_ms(),
        now_ms().saturating_add(IDEMPOTENCY_TTL_MS),
    )
}

fn complete_mutation(
    storage: &PlatformStorage,
    account_id: AccountId,
    scope: &str,
    key: &str,
    fingerprint: &[u8; 32],
    queue_id: QueueId,
    value: &impl Serialize,
) -> Result<MutationOutcome, PlatformError> {
    let response = serde_json::to_vec(value).map_err(|_| internal())?;
    WorkerRepository::new(storage.db()).complete_idempotency_with_queue_ref(
        account_id,
        scope,
        key,
        fingerprint,
        &response,
        queue_id,
    )?;
    Ok(MutationOutcome::Applied(response))
}

fn persist_failure(
    storage: &PlatformStorage,
    account_id: AccountId,
    scope: &str,
    key: &str,
    fingerprint: &[u8; 32],
    code: ErrorCode,
) -> Result<(), PlatformError> {
    let response = serde_json::to_vec(&serde_json::json!({ "code": code.as_str() }))
        .map_err(|_| internal())?;
    WorkerRepository::new(storage.db()).fail_idempotency(
        account_id,
        scope,
        key,
        fingerprint,
        &response,
    )
}

fn is_final_mutation_failure(code: ErrorCode) -> bool {
    matches!(
        code,
        ErrorCode::QueueNotFound
            | ErrorCode::QueueNameConflict
            | ErrorCode::QueueNotReady
            | ErrorCode::QueueConfigPending
            | ErrorCode::QueueReferenced
            | ErrorCode::QueueNotEmpty
            | ErrorCode::ConfigInvalid
            | ErrorCode::LimitInvalid
            | ErrorCode::QuotaExceeded
    )
}

fn mutation_fingerprint(
    storage: &PlatformStorage,
    operation: &[u8],
    queue_id: QueueId,
    canonical: &[u8],
) -> [u8; 32] {
    let mut bytes = Vec::with_capacity(API_VERSION.len() + operation.len() + 16 + canonical.len());
    bytes.extend_from_slice(API_VERSION);
    bytes.extend_from_slice(operation);
    bytes.extend_from_slice(queue_id.as_uuid().as_bytes());
    bytes.extend_from_slice(canonical);
    storage.crypto().fingerprint_request(&bytes)
}

fn mutation_response(
    result: Result<Result<MutationOutcome, PlatformError>, tokio::task::JoinError>,
    request_id: RequestId,
    applied_status: StatusCode,
) -> Response {
    match result {
        Ok(Ok(MutationOutcome::Applied(bytes))) => json_bytes(bytes, applied_status),
        Ok(Ok(MutationOutcome::Replay(bytes))) => json_bytes(bytes, StatusCode::OK),
        Ok(Ok(MutationOutcome::Failed(bytes))) => json_bytes(bytes, StatusCode::CONFLICT),
        Ok(Err(error)) => error_response(error, request_id),
        Err(_) => error_response(internal(), request_id),
    }
}

fn parse_list_query(query: Option<&str>) -> Result<(Option<(i64, QueueId)>, u32), PlatformError> {
    let mut cursor = None;
    let mut limit = 100_u32;
    let mut limit_seen = false;
    let mut cursor_seen = false;
    for pair in query
        .unwrap_or("")
        .split('&')
        .filter(|pair| !pair.is_empty())
    {
        let Some((name, value)) = pair.split_once('=') else {
            return Err(invalid_query());
        };
        match name {
            "limit" if !limit_seen => {
                limit = value.parse().map_err(|_| invalid_query())?;
                limit_seen = true;
            }
            "cursor" if !cursor_seen => {
                cursor = Some(parse_cursor(value)?);
                cursor_seen = true;
            }
            _ => return Err(invalid_query()),
        }
    }
    if limit == 0 || limit > 1000 {
        return Err(invalid_query());
    }
    Ok((cursor, limit))
}

fn queue_cursor(queue: &open_compute_storage::QueueRecord) -> String {
    URL_SAFE_NO_PAD.encode(format!("{}:{}", queue.created_at_ms, queue.id))
}

fn parse_cursor(value: &str) -> Result<(i64, QueueId), PlatformError> {
    let decoded = URL_SAFE_NO_PAD.decode(value).map_err(|_| invalid_query())?;
    let decoded = std::str::from_utf8(&decoded).map_err(|_| invalid_query())?;
    let (created, id) = decoded.split_once(':').ok_or_else(invalid_query)?;
    Ok((
        created.parse().map_err(|_| invalid_query())?,
        QueueId::from_str(id).map_err(|_| invalid_query())?,
    ))
}

fn parse_force(query: Option<&str>) -> Result<bool, PlatformError> {
    match query {
        None | Some("") | Some("force=false") => Ok(false),
        Some("force=true") => Ok(true),
        _ => Err(invalid_query()),
    }
}

fn expected_lifecycle_generation(request: &Request) -> Result<u64, PlatformError> {
    request
        .headers()
        .get(EXPECTED_LIFECYCLE_HEADER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .ok_or_else(|| {
            PlatformError::new(
                ErrorCode::ConfigInvalid,
                "an expected Queue lifecycle generation is required",
            )
        })
}

fn authorized_api<'a>(state: &'a HttpState, request: &Request) -> Option<&'a Arc<QueueApiState>> {
    authorize(state, request)
        .then(|| state.queue_api())
        .flatten()
}

fn unauthorized_or_unavailable(
    state: &HttpState,
    request: &Request,
    request_id: RequestId,
) -> Response {
    if authorize(state, request) {
        StatusCode::SERVICE_UNAVAILABLE.into_response()
    } else {
        error_response(
            PlatformError::new(
                ErrorCode::AdminAuthRequired,
                "admin authentication is required",
            ),
            request_id,
        )
    }
}

async fn read_json<T: for<'de> Deserialize<'de>>(request: Request) -> Result<T, PlatformError> {
    let bytes = to_bytes(request.into_body(), MAX_JSON_BODY)
        .await
        .map_err(|_| {
            PlatformError::new(ErrorCode::LimitInvalid, "Queue request body is too large")
        })?;
    serde_json::from_slice(&bytes)
        .map_err(|_| PlatformError::new(ErrorCode::ConfigInvalid, "Queue request JSON is invalid"))
}

fn idempotency_key(request: &Request) -> Result<String, PlatformError> {
    let key = request
        .headers()
        .get(IDEMPOTENCY_HEADER)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    if key.is_empty()
        || key.len() > 128
        || key
            .bytes()
            .any(|byte| byte.is_ascii_control() || byte.is_ascii_whitespace())
    {
        return Err(PlatformError::new(
            ErrorCode::ConfigInvalid,
            "a bounded Idempotency-Key is required",
        ));
    }
    Ok(key.to_owned())
}

fn parse_account(value: &str) -> Result<AccountId, PlatformError> {
    AccountId::from_str(value)
        .map_err(|_| PlatformError::new(ErrorCode::ConfigInvalid, "account ID is invalid"))
}

fn parse_ids(account: &str, queue: &str) -> Result<(AccountId, QueueId), PlatformError> {
    Ok((
        parse_account(account)?,
        QueueId::from_str(queue)
            .map_err(|_| PlatformError::new(ErrorCode::ConfigInvalid, "Queue ID is invalid"))?,
    ))
}

fn request_id(request: &Request) -> RequestId {
    request
        .extensions()
        .get::<RequestId>()
        .copied()
        .unwrap_or_else(RequestId::generate)
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| {
            i64::try_from(duration.as_millis()).unwrap_or(i64::MAX)
        })
}

fn json_response(value: &impl Serialize, status: StatusCode) -> Response {
    serde_json::to_vec(value).map_or_else(
        |_| error_response(internal(), RequestId::generate()),
        |bytes| json_bytes(bytes, status),
    )
}

fn json_bytes(bytes: Vec<u8>, status: StatusCode) -> Response {
    (
        status,
        [(axum::http::header::CONTENT_TYPE, "application/json")],
        Body::from(bytes),
    )
        .into_response()
}

#[allow(clippy::needless_pass_by_value)]
fn error_response(error: PlatformError, request_id: RequestId) -> Response {
    let code = error.code();
    let status = match code {
        ErrorCode::QueueNotFound => StatusCode::NOT_FOUND,
        ErrorCode::AdminAuthRequired => StatusCode::UNAUTHORIZED,
        ErrorCode::BindingPermissionDenied => StatusCode::FORBIDDEN,
        ErrorCode::QueueNameConflict
        | ErrorCode::QueueNotReady
        | ErrorCode::QueueConfigPending
        | ErrorCode::QueueReferenced
        | ErrorCode::QueueNotEmpty
        | ErrorCode::IdempotencyConflict => StatusCode::CONFLICT,
        ErrorCode::ConfigInvalid
        | ErrorCode::LimitInvalid
        | ErrorCode::QueueDelayInvalid
        | ErrorCode::QueueInvalidMessage
        | ErrorCode::QueueBatchLimitExceeded => StatusCode::BAD_REQUEST,
        ErrorCode::QueueMessageTooLarge => StatusCode::PAYLOAD_TOO_LARGE,
        ErrorCode::QueueBacklogLimitExceeded
        | ErrorCode::QuotaExceeded
        | ErrorCode::AdmissionBusy => StatusCode::TOO_MANY_REQUESTS,
        ErrorCode::StoragePressure | ErrorCode::DiskHardLimit => StatusCode::INSUFFICIENT_STORAGE,
        ErrorCode::QueueStorageUnavailable | ErrorCode::PlatformUnavailable => {
            StatusCode::SERVICE_UNAVAILABLE
        }
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    };
    let mut response = (
        status,
        axum::Json(serde_json::json!({
            "ok": false,
            "error": {
                "code": code.as_str(),
                "message": "Queue control request failed",
                "requestId": request_id,
            }
        })),
    )
        .into_response();
    response.extensions_mut().insert(ProductErrorCode(code));
    response
}

fn invalid_query() -> PlatformError {
    PlatformError::new(ErrorCode::ConfigInvalid, "Queue control query is invalid")
}

fn idempotency_running() -> PlatformError {
    PlatformError::new(
        ErrorCode::IdempotencyConflict,
        "Queue mutation is already running",
    )
}

fn internal() -> PlatformError {
    PlatformError::new(ErrorCode::Internal, "Queue control operation failed")
}

#[cfg(test)]
#[path = "queue_http_tests.rs"]
mod tests;
