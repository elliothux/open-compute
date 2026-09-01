//! Private generation-authenticated Queue producer backend.

use crate::metrics::{MetricsRegistry, QueueMetricOperation};
use axum::body::{Body, to_bytes};
use axum::extract::Request;
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse as _, Response};
use open_compute_core::{BindingId, DeploymentId, ErrorCode, PlatformError, QueuesConfig};
use open_compute_storage::{
    QUEUE_MAX_BATCH_BYTES, QUEUE_MAX_BATCH_MESSAGES, QUEUE_MAX_DELAY_SECONDS,
    QUEUE_MAX_MESSAGE_BYTES, QueueContentType, QueueEnqueueRequest, QueueMessageInput,
    QueueRepository, SchedulerStore,
};
use std::str::FromStr as _;
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};
use uuid::Uuid;

const DEPLOYMENT_HEADER: &str = "x-open-compute-deployment-id";
const DESCRIPTOR_HEADER: &str = "x-open-compute-descriptor-sha256";
const REQUEST_HEADER: &str = "x-open-compute-request-id";
const OUTPUT_GATE_HEADER: &str = "x-open-compute-output-gate";
const ERROR_HEADER: &str = "x-open-compute-error-code";
const FRAME_CONTENT_TYPE: &str = "application/vnd.open-compute.queue.v1+frame";
const MAX_FRAME_BYTES: usize = 256_000 + 1024;
const TIMEOUT: Duration = Duration::from_secs(30);

#[cfg(any(test, feature = "test-support"))]
static ENQUEUE_HOLD: Mutex<Option<Arc<QueueEnqueueHold>>> = Mutex::new(None);

/// Test-only admission gate used to freeze Queue enqueue around a process death.
#[cfg(any(test, feature = "test-support"))]
#[derive(Debug)]
pub struct QueueEnqueueHold {
    allow_before: tokio::sync::watch::Sender<bool>,
    allow_after: tokio::sync::watch::Sender<bool>,
    seen: tokio::sync::watch::Sender<u32>,
}

#[cfg(any(test, feature = "test-support"))]
impl QueueEnqueueHold {
    /// Create a hold that currently admits enqueue.
    #[must_use]
    pub fn new() -> Arc<Self> {
        let (allow_before, _) = tokio::sync::watch::channel(true);
        let (allow_after, _) = tokio::sync::watch::channel(true);
        let (seen, _) = tokio::sync::watch::channel(0);
        Arc::new(Self {
            allow_before,
            allow_after,
            seen,
        })
    }

    /// Install this hold for subsequent private Queue producer requests.
    pub fn install(self: &Arc<Self>) {
        *ENQUEUE_HOLD
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(self.clone());
    }

    /// Remove any installed hold.
    pub fn clear() {
        *ENQUEUE_HOLD
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    }

    /// Block enqueue before durable admission.
    pub fn block_before(&self) {
        let _ = self.allow_before.send(false);
    }

    /// Allow enqueue before durable admission.
    pub fn release_before(&self) {
        let _ = self.allow_before.send(true);
    }

    /// Block the producer response after durable admission.
    pub fn block_after(&self) {
        let _ = self.allow_after.send(false);
    }

    /// Allow the producer response after durable admission.
    pub fn release_after(&self) {
        let _ = self.allow_after.send(true);
    }

    /// Current observed enqueue-attempt count.
    #[must_use]
    pub fn seen(&self) -> u32 {
        *self.seen.borrow()
    }

    /// Wait until the backend has observed `count` enqueue attempts.
    pub async fn wait_seen(&self, count: u32) {
        let mut rx = self.seen.subscribe();
        while *rx.borrow() < count {
            rx.changed().await.expect("enqueue hold closed");
        }
    }

    async fn wait_before(&self) {
        let mut rx = self.allow_before.subscribe();
        while !*rx.borrow() {
            if rx.changed().await.is_err() {
                break;
            }
        }
    }

    async fn wait_after(&self) {
        let mut rx = self.allow_after.subscribe();
        while !*rx.borrow() {
            if rx.changed().await.is_err() {
                break;
            }
        }
    }

    fn mark_seen(&self) {
        self.seen
            .send_modify(|value| *value = value.saturating_add(1));
    }
}

#[cfg(any(test, feature = "test-support"))]
fn current_hold() -> Option<Arc<QueueEnqueueHold>> {
    ENQUEUE_HOLD
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
}

/// Composed Queue control authorization and durable scheduler authority.
#[derive(Clone, Debug)]
pub struct QueueBindingService {
    storage: Arc<open_compute_storage::PlatformStorage>,
    scheduler: Arc<SchedulerStore>,
    metrics: Option<Arc<MetricsRegistry>>,
    concurrency: QueueConcurrencyBudget,
}

impl QueueBindingService {
    /// Bind the private Queue producer backend.
    #[must_use]
    pub fn new(
        storage: Arc<open_compute_storage::PlatformStorage>,
        scheduler: Arc<SchedulerStore>,
    ) -> Self {
        let config = QueuesConfig::default();
        Self {
            storage,
            scheduler,
            metrics: None,
            concurrency: QueueConcurrencyBudget::new(
                config.max_in_flight_requests,
                config.max_in_flight_requests_per_binding,
            ),
        }
    }

    /// Attach the fixed low-cardinality Queue metrics registry.
    #[must_use]
    pub fn with_metrics(mut self, metrics: Arc<MetricsRegistry>) -> Self {
        self.metrics = Some(metrics);
        self
    }

    /// Apply validated operator Queue request-admission limits.
    #[must_use]
    pub fn with_concurrency_limits(mut self, global: u32, per_binding: u32) -> Self {
        self.concurrency = QueueConcurrencyBudget::new(global, per_binding);
        self
    }

    /// Handle one already generation-authenticated private request.
    pub async fn handle(&self, request: Request) -> Response {
        let Some((binding_id, operation)) = parse_path(request.uri().path()) else {
            return error(ErrorCode::QueueInvariantViolation, StatusCode::NOT_FOUND);
        };
        let started = Instant::now();
        let _lease = match self.concurrency.acquire(binding_id).await {
            Ok(lease) => lease,
            Err(error) => {
                self.observe(operation, false, 0, 0, started);
                return platform_error(&error);
            }
        };
        if request.method() != axum::http::Method::POST {
            self.observe(operation, false, 0, 0, started);
            return error(ErrorCode::QueueInvariantViolation, StatusCode::BAD_REQUEST);
        }
        let Some(request_id) = parse_request_id(request.headers()) else {
            self.observe(operation, false, 0, 0, started);
            return error(ErrorCode::QueueInvariantViolation, StatusCode::BAD_REQUEST);
        };
        let output_gate = match header_text(request.headers(), OUTPUT_GATE_HEADER) {
            Some("0") => false,
            Some("1") => true,
            _ => {
                self.observe(operation, false, 0, 0, started);
                return error(ErrorCode::QueueInvariantViolation, StatusCode::BAD_REQUEST);
            }
        };
        let Some(deployment_id) = header_text(request.headers(), DEPLOYMENT_HEADER)
            .and_then(|value| DeploymentId::from_str(value).ok())
        else {
            self.observe(operation, false, 0, 0, started);
            return error(ErrorCode::QueueInvariantViolation, StatusCode::BAD_REQUEST);
        };
        let Some(descriptor) = parse_digest(request.headers()) else {
            self.observe(operation, false, 0, 0, started);
            return error(ErrorCode::QueueInvariantViolation, StatusCode::BAD_REQUEST);
        };
        if operation == QueueOperation::Metrics {
            if output_gate || !content_type_is(request.headers(), "application/json") {
                self.observe(operation, false, 0, 0, started);
                return error(
                    ErrorCode::QueueInvariantViolation,
                    StatusCode::UNSUPPORTED_MEDIA_TYPE,
                );
            }
            return self
                .read_metrics(binding_id, deployment_id, descriptor, started)
                .await;
        }
        if operation == QueueOperation::Finalize {
            if !output_gate || !content_type_is(request.headers(), "application/json") {
                return error(
                    ErrorCode::QueueInvariantViolation,
                    StatusCode::UNSUPPORTED_MEDIA_TYPE,
                );
            }
            return self
                .finalize(binding_id, deployment_id, descriptor, request_id)
                .await;
        }
        if !content_type_is(request.headers(), FRAME_CONTENT_TYPE) {
            self.observe(operation, false, 0, 0, started);
            return error(
                ErrorCode::QueueInvariantViolation,
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
            );
        }
        let declared = request
            .headers()
            .get(header::CONTENT_LENGTH)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<usize>().ok());
        if declared.is_some_and(|length| length > MAX_FRAME_BYTES) {
            self.observe(operation, false, 0, declared.unwrap_or(0) as u64, started);
            return error(
                ErrorCode::QueueBatchLimitExceeded,
                StatusCode::PAYLOAD_TOO_LARGE,
            );
        }
        let Ok(bytes) = to_bytes(request.into_body(), MAX_FRAME_BYTES).await else {
            self.observe(operation, false, 0, MAX_FRAME_BYTES as u64, started);
            return error(
                ErrorCode::QueueBatchLimitExceeded,
                StatusCode::PAYLOAD_TOO_LARGE,
            );
        };
        let frame = match parse_frame(&bytes, operation) {
            Ok(value) => value,
            Err(value) => {
                self.observe(operation, false, 0, bytes.len() as u64, started);
                return platform_error(&value);
            }
        };
        let message_count = u64::try_from(frame.messages.len()).unwrap_or(u64::MAX);
        let body_bytes = frame.messages.iter().fold(0_u64, |total, message| {
            total.saturating_add(u64::try_from(message.body.len()).unwrap_or(u64::MAX))
        });
        let storage = self.storage.clone();
        let scheduler = self.scheduler.clone();
        let admission_bytes = frame.messages.iter().fold(64 * 1024_u64, |sum, message| {
            sum.saturating_add(u64::try_from(message.body.len()).unwrap_or(u64::MAX))
        });
        #[cfg(any(test, feature = "test-support"))]
        if let Some(hold) = current_hold() {
            hold.mark_seen();
            hold.wait_before().await;
        }
        let task = tokio::task::spawn_blocking(move || {
            let authorized = QueueRepository::new(storage.db()).authorize(
                binding_id,
                deployment_id,
                &descriptor,
            )?;
            let _admission = storage.reserve_mutation(admission_bytes)?;
            scheduler.enqueue_queue(
                &QueueEnqueueRequest {
                    queue_id: authorized.queue.id,
                    request_id,
                    output_gate,
                    lifecycle_generation: authorized.binding.queue_lifecycle_generation,
                    config_generation: authorized.queue.config_generation,
                    batch_delay_seconds: frame.batch_delay_seconds,
                    messages: frame.messages,
                },
                unix_ms(),
            )
        });
        match tokio::time::timeout(TIMEOUT, task).await {
            Ok(Ok(Ok(result))) => {
                #[cfg(any(test, feature = "test-support"))]
                if let Some(hold) = current_hold() {
                    hold.wait_after().await;
                }
                self.observe(operation, true, message_count, body_bytes, started);
                json(&result.metrics)
            }
            Ok(Ok(Err(value))) => {
                self.observe(operation, false, message_count, body_bytes, started);
                platform_error(&value)
            }
            Ok(Err(_)) => {
                self.observe(operation, false, message_count, body_bytes, started);
                error(
                    ErrorCode::QueueStorageUnavailable,
                    StatusCode::SERVICE_UNAVAILABLE,
                )
            }
            Err(_) => {
                self.observe(operation, false, message_count, body_bytes, started);
                if let Some(metrics) = &self.metrics
                    && let Some(metric) = operation.metric()
                {
                    metrics.inc_queue_result_unknown(metric);
                }
                error(
                    ErrorCode::QueueSendResultUnknown,
                    StatusCode::SERVICE_UNAVAILABLE,
                )
            }
        }
    }

    async fn read_metrics(
        &self,
        binding_id: BindingId,
        deployment_id: DeploymentId,
        descriptor: [u8; 32],
        started: Instant,
    ) -> Response {
        let storage = self.storage.clone();
        let scheduler = self.scheduler.clone();
        let task = tokio::task::spawn_blocking(move || {
            let authorized = QueueRepository::new(storage.db()).authorize(
                binding_id,
                deployment_id,
                &descriptor,
            )?;
            scheduler.queue_metrics(
                authorized.queue.id,
                authorized.binding.queue_lifecycle_generation,
                authorized.queue.config_generation,
            )
        });
        match tokio::time::timeout(TIMEOUT, task).await {
            Ok(Ok(Ok(metrics))) => {
                self.observe(QueueOperation::Metrics, true, 0, 0, started);
                json(&metrics)
            }
            Ok(Ok(Err(value))) => {
                self.observe(QueueOperation::Metrics, false, 0, 0, started);
                platform_error(&value)
            }
            _ => {
                self.observe(QueueOperation::Metrics, false, 0, 0, started);
                error(
                    ErrorCode::QueueStorageUnavailable,
                    StatusCode::SERVICE_UNAVAILABLE,
                )
            }
        }
    }

    async fn finalize(
        &self,
        binding_id: BindingId,
        deployment_id: DeploymentId,
        descriptor: [u8; 32],
        request_id: Uuid,
    ) -> Response {
        let storage = self.storage.clone();
        let scheduler = self.scheduler.clone();
        let task = tokio::task::spawn_blocking(move || {
            let authorized = QueueRepository::new(storage.db()).authorize(
                binding_id,
                deployment_id,
                &descriptor,
            )?;
            scheduler.finalize_queue_enqueue(
                request_id,
                authorized.queue.id,
                authorized.binding.queue_lifecycle_generation,
                unix_ms(),
            )
        });
        match tokio::time::timeout(TIMEOUT, task).await {
            Ok(Ok(Ok(()))) => json(&serde_json::json!({})),
            Ok(Ok(Err(value))) => platform_error(&value),
            _ => error(
                ErrorCode::QueueStorageUnavailable,
                StatusCode::SERVICE_UNAVAILABLE,
            ),
        }
    }

    fn observe(
        &self,
        operation: QueueOperation,
        success: bool,
        messages: u64,
        body_bytes: u64,
        started: Instant,
    ) {
        if let (Some(metrics), Some(operation)) = (&self.metrics, operation.metric()) {
            metrics.observe_queue_producer(
                operation,
                success,
                messages,
                body_bytes,
                started.elapsed(),
            );
        }
    }
}

#[derive(Clone, Debug)]
struct QueueConcurrencyBudget {
    global: Arc<tokio::sync::Semaphore>,
    per_binding: usize,
    bindings: Arc<Mutex<std::collections::HashMap<BindingId, Weak<tokio::sync::Semaphore>>>>,
}

impl QueueConcurrencyBudget {
    fn new(global: u32, per_binding: u32) -> Self {
        Self {
            global: Arc::new(tokio::sync::Semaphore::new(
                usize::try_from(global).unwrap_or(usize::MAX),
            )),
            per_binding: usize::try_from(per_binding).unwrap_or(usize::MAX),
            bindings: Arc::new(Mutex::new(std::collections::HashMap::new())),
        }
    }

    async fn acquire(&self, binding_id: BindingId) -> Result<QueueConcurrencyLease, PlatformError> {
        let binding = {
            let mut bindings = self
                .bindings
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            bindings.retain(|_, gate| gate.strong_count() > 0);
            if let Some(gate) = bindings.get(&binding_id).and_then(Weak::upgrade) {
                gate
            } else {
                let gate = Arc::new(tokio::sync::Semaphore::new(self.per_binding));
                bindings.insert(binding_id, Arc::downgrade(&gate));
                gate
            }
        };
        let binding = tokio::time::timeout(TIMEOUT, binding.acquire_owned())
            .await
            .map_err(|_| queue_busy())?
            .map_err(|_| queue_busy())?;
        let global = tokio::time::timeout(TIMEOUT, self.global.clone().acquire_owned())
            .await
            .map_err(|_| queue_busy())?
            .map_err(|_| queue_busy())?;
        Ok(QueueConcurrencyLease {
            _global: global,
            _binding: binding,
        })
    }
}

#[derive(Debug)]
struct QueueConcurrencyLease {
    _global: tokio::sync::OwnedSemaphorePermit,
    _binding: tokio::sync::OwnedSemaphorePermit,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum QueueOperation {
    Send,
    Batch,
    Metrics,
    Finalize,
}

impl QueueOperation {
    const fn metric(self) -> Option<QueueMetricOperation> {
        match self {
            Self::Send => Some(QueueMetricOperation::Send),
            Self::Batch => Some(QueueMetricOperation::Batch),
            Self::Metrics => Some(QueueMetricOperation::Metrics),
            Self::Finalize => None,
        }
    }
}

#[derive(Debug)]
struct ParsedFrame {
    batch_delay_seconds: Option<u32>,
    messages: Vec<QueueMessageInput>,
}

fn parse_path(path: &str) -> Option<(BindingId, QueueOperation)> {
    let rest = path.strip_prefix("/internal/bindings/v1/queue/")?;
    let (id, operation) = rest.split_once('/')?;
    if operation.contains('/') {
        return None;
    }
    let operation = match operation {
        "send" => QueueOperation::Send,
        "batch" => QueueOperation::Batch,
        "metrics" => QueueOperation::Metrics,
        "finalize" => QueueOperation::Finalize,
        _ => return None,
    };
    Some((BindingId::from_str(id).ok()?, operation))
}

fn parse_frame(bytes: &[u8], operation: QueueOperation) -> Result<ParsedFrame, PlatformError> {
    if bytes.len() < 11 || &bytes[..4] != b"OCQ1" {
        return Err(protocol_error());
    }
    let encoded_operation = bytes[4];
    if (operation == QueueOperation::Send && encoded_operation != 1)
        || (operation == QueueOperation::Batch && encoded_operation != 2)
    {
        return Err(protocol_error());
    }
    let count = usize::from(u16::from_be_bytes([bytes[5], bytes[6]]));
    if count == 0
        || count > usize::try_from(QUEUE_MAX_BATCH_MESSAGES).unwrap_or(usize::MAX)
        || (operation == QueueOperation::Send && count != 1)
    {
        return Err(PlatformError::new(
            ErrorCode::QueueBatchLimitExceeded,
            "Queue frame message count is outside the supported range",
        ));
    }
    let batch_delay_seconds = decode_delay(i32::from_be_bytes(
        bytes[7..11].try_into().map_err(|_| protocol_error())?,
    ))?;
    let mut offset = 11_usize;
    let mut total = 0_u64;
    let mut messages = Vec::with_capacity(count);
    for _ in 0..count {
        let end = offset.checked_add(9).ok_or_else(protocol_error)?;
        if end > bytes.len() {
            return Err(protocol_error());
        }
        let content_type = match bytes[offset] {
            1 => QueueContentType::Json,
            2 => QueueContentType::Text,
            3 => QueueContentType::Bytes,
            4 => QueueContentType::V8,
            _ => {
                return Err(PlatformError::new(
                    ErrorCode::QueueContentTypeUnsupported,
                    "Queue frame content type is unsupported",
                ));
            }
        };
        let delay_seconds = decode_delay(i32::from_be_bytes(
            bytes[offset + 1..offset + 5]
                .try_into()
                .map_err(|_| protocol_error())?,
        ))?;
        let length = usize::try_from(u32::from_be_bytes(
            bytes[offset + 5..end]
                .try_into()
                .map_err(|_| protocol_error())?,
        ))
        .map_err(|_| protocol_error())?;
        if u64::try_from(length).map_err(|_| protocol_error())? > QUEUE_MAX_MESSAGE_BYTES {
            return Err(PlatformError::new(
                ErrorCode::QueueMessageTooLarge,
                "Queue frame message exceeds 128000 bytes",
            ));
        }
        let body_start = end;
        let body_end = body_start.checked_add(length).ok_or_else(protocol_error)?;
        if body_end > bytes.len() {
            return Err(protocol_error());
        }
        total = total
            .checked_add(u64::try_from(length).map_err(|_| protocol_error())?)
            .ok_or_else(protocol_error)?;
        if total > QUEUE_MAX_BATCH_BYTES {
            return Err(PlatformError::new(
                ErrorCode::QueueBatchLimitExceeded,
                "Queue frame batch body limit exceeded",
            ));
        }
        messages.push(QueueMessageInput {
            content_type,
            body: bytes[body_start..body_end].to_vec(),
            delay_seconds,
        });
        offset = body_end;
    }
    if offset != bytes.len() {
        return Err(protocol_error());
    }
    Ok(ParsedFrame {
        batch_delay_seconds,
        messages,
    })
}

fn decode_delay(value: i32) -> Result<Option<u32>, PlatformError> {
    if value == -1 {
        return Ok(None);
    }
    let value = u32::try_from(value).map_err(|_| delay_error())?;
    if value > QUEUE_MAX_DELAY_SECONDS {
        return Err(delay_error());
    }
    Ok(Some(value))
}

fn json(value: &impl serde::Serialize) -> Response {
    match serde_json::to_vec(value) {
        Ok(bytes) => {
            let mut response = Response::new(Body::from(bytes));
            response.headers_mut().insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/json"),
            );
            response
        }
        Err(_) => error(
            ErrorCode::QueueInvariantViolation,
            StatusCode::INTERNAL_SERVER_ERROR,
        ),
    }
}

fn platform_error(value: &PlatformError) -> Response {
    let status = match value.code() {
        ErrorCode::QueueNotFound => StatusCode::NOT_FOUND,
        ErrorCode::QueueNotReady
        | ErrorCode::QueueConfigPending
        | ErrorCode::QueueReferenced
        | ErrorCode::QueueNotEmpty
        | ErrorCode::IdempotencyConflict => StatusCode::CONFLICT,
        ErrorCode::QueueMessageTooLarge | ErrorCode::QueueBatchLimitExceeded => {
            StatusCode::PAYLOAD_TOO_LARGE
        }
        ErrorCode::QueueContentTypeUnsupported
        | ErrorCode::QueueInvalidMessage
        | ErrorCode::QueueDelayInvalid => StatusCode::BAD_REQUEST,
        ErrorCode::QueueBacklogLimitExceeded => StatusCode::TOO_MANY_REQUESTS,
        ErrorCode::QueueStorageUnavailable
        | ErrorCode::QueueSendResultUnknown
        | ErrorCode::StoragePressure
        | ErrorCode::AdmissionBusy => StatusCode::SERVICE_UNAVAILABLE,
        _ => StatusCode::UNPROCESSABLE_ENTITY,
    };
    error(value.code(), status)
}

fn error(code: ErrorCode, status: StatusCode) -> Response {
    let mut response = (
        status,
        axum::Json(serde_json::json!({
            "ok": false,
            "error": {
                "code": code.as_str(),
                "resultUnknown": code == ErrorCode::QueueSendResultUnknown,
            }
        })),
    )
        .into_response();
    if let Ok(value) = HeaderValue::from_str(code.as_str()) {
        response
            .headers_mut()
            .insert(HeaderName::from_static(ERROR_HEADER), value);
    }
    response
}

fn parse_digest(headers: &HeaderMap) -> Option<[u8; 32]> {
    let value = header_text(headers, DESCRIPTOR_HEADER)?;
    hex::decode(value).ok()?.try_into().ok()
}

fn parse_request_id(headers: &HeaderMap) -> Option<Uuid> {
    let value = header_text(headers, REQUEST_HEADER)?;
    let parsed = Uuid::parse_str(value).ok()?;
    (parsed.hyphenated().to_string() == value).then_some(parsed)
}

fn content_type_is(headers: &HeaderMap, expected: &str) -> bool {
    header_text(headers, header::CONTENT_TYPE.as_str())
        .and_then(|value| value.split(';').next())
        .is_some_and(|value| value.trim() == expected)
}

fn header_text<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name)?.to_str().ok()
}

fn protocol_error() -> PlatformError {
    PlatformError::new(
        ErrorCode::QueueInvariantViolation,
        "Queue private frame is invalid",
    )
}

fn queue_busy() -> PlatformError {
    PlatformError::new(
        ErrorCode::AdmissionBusy,
        "Queue producer concurrency budget is exhausted",
    )
}

fn delay_error() -> PlatformError {
    PlatformError::new(
        ErrorCode::QueueDelayInvalid,
        "Queue delay is outside 0..86400",
    )
}

fn unix_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
        .unwrap_or(i64::MAX)
}

#[cfg(test)]
#[path = "queue_backend_tests.rs"]
mod tests;
