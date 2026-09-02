//! Authorized private backend for the latest Vectorize Workers facade.

use axum::body::to_bytes;
use axum::extract::Request;
use axum::http::{HeaderMap, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use open_compute_core::{
    BindingId, BindingKind, DeploymentId, ErrorCode, PlatformError, ResourceId,
};
use open_compute_search::{
    ExactCandidate, ExactTopK, MAX_TOP_K, MAX_TOP_K_WITH_VALUES, compile_filter, validate_metadata,
};
use open_compute_storage::{
    AuthorizedBinding, BindingRepository, PlatformStorage, VectorMutationInput, VectorMutationKind,
    VectorRecord, VectorizeEngine, VectorizeIndexRepository, VectorizePaths,
};
use open_compute_workers::{ResourcePin, ResourcePins};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::str::FromStr;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

mod warm_cache;
use warm_cache::load_warm_snapshot;

const JSON_CALL_PATH: &str = "/internal/vectorize/v1/call";
const MUTATE_PREFIX: &str = "/internal/vectorize/v1/mutate/";
const FRAME_CONTENT_TYPE: &str = "application/vnd.open-compute.vectorize.v1+frame";
const MAX_JSON_BYTES: usize = 256 * 1024;
const MAX_FRAME_BYTES: usize = 24 * 1024 * 1024;
const QUERY_DEADLINE: Duration = Duration::from_secs(15);
const QUERY_THREADS: usize = 4;
const QUERY_ADMISSION: usize = 8;

static VECTOR_QUERY_POOL: OnceLock<Option<rayon::ThreadPool>> = OnceLock::new();
static VECTOR_QUERY_SEMAPHORE: OnceLock<Arc<tokio::sync::Semaphore>> = OnceLock::new();

/// Fully composed Vectorize executor over durable per-index SQLite authority.
#[derive(Clone, Debug)]
pub struct VectorizeBindingService {
    storage: Arc<PlatformStorage>,
    pins: ResourcePins,
    metrics: Option<Arc<crate::metrics::MetricsRegistry>>,
}

impl VectorizeBindingService {
    /// Bind storage authority and lifecycle pins.
    #[must_use]
    pub const fn new(storage: Arc<PlatformStorage>, pins: ResourcePins) -> Self {
        Self {
            storage,
            pins,
            metrics: None,
        }
    }

    /// Attach the process fixed-series metrics registry.
    #[must_use]
    pub fn with_metrics(mut self, metrics: Arc<crate::metrics::MetricsRegistry>) -> Self {
        self.metrics = Some(metrics);
        self
    }

    /// Handle one generation-authenticated request from the shared private listener.
    pub async fn handle(&self, request: Request) -> Response {
        let mutation = request.uri().path().starts_with(MUTATE_PREFIX);
        let result = self.handle_inner(request).await;
        if let Some(metrics) = &self.metrics {
            metrics.observe_vectorize_request(mutation, result.is_ok());
        }
        match result {
            Ok(response) => response,
            Err(error) => error_response(&error),
        }
    }

    async fn handle_inner(&self, request: Request) -> Result<Response, PlatformError> {
        if request.method() != Method::POST {
            return Err(protocol_error());
        }
        let path = request.uri().path().to_string();
        let binding = self.authorize(request.headers())?;
        let pin = self.pins.try_pin(binding.resource.id)?;
        if path == JSON_CALL_PATH {
            if !content_type_is(request.headers(), "application/json") {
                return Err(protocol_error());
            }
            let body = to_bytes(request.into_body(), MAX_JSON_BYTES)
                .await
                .map_err(|_| limit_error())?;
            let call: JsonCall = serde_json::from_slice(&body).map_err(|_| protocol_error())?;
            self.execute_json(binding, pin, call).await
        } else if let Some(operation) = path.strip_prefix(MUTATE_PREFIX) {
            if !content_type_is(request.headers(), FRAME_CONTENT_TYPE) {
                return Err(protocol_error());
            }
            let kind = match operation {
                "insert" => VectorMutationKind::Insert,
                "upsert" => VectorMutationKind::Upsert,
                _ => return Err(protocol_error()),
            };
            if !binding.binding.permissions.write {
                return Err(permission_denied());
            }
            let body = to_bytes(request.into_body(), MAX_FRAME_BYTES)
                .await
                .map_err(|_| limit_error())?;
            let items = decode_mutation_frame(&body, kind)?;
            self.enqueue(binding, pin, kind, items).await
        } else {
            Err(protocol_error())
        }
    }

    fn authorize(&self, headers: &HeaderMap) -> Result<AuthorizedBinding, PlatformError> {
        let binding_id = parse_header::<BindingId>(headers, "x-open-compute-binding-id")?;
        let deployment_id = parse_header::<DeploymentId>(headers, "x-open-compute-deployment-id")?;
        let resource_id = parse_header::<ResourceId>(headers, "x-open-compute-resource-id")?;
        let generation = header_text(headers, "x-open-compute-resource-generation")?
            .parse::<u64>()
            .map_err(|_| protocol_error())?;
        let descriptor = parse_digest(headers, "x-open-compute-descriptor-sha256")?;
        let _: open_compute_core::RequestId = parse_header(headers, "x-open-compute-request-id")?;
        let binding = BindingRepository::new(self.storage.db()).authorize(
            binding_id,
            deployment_id,
            &descriptor,
        )?;
        if binding.binding.kind != BindingKind::VectorizeIndex
            || binding.binding.capability_version != 1
            || binding.resource.id != resource_id
            || binding.resource.spec_generation != generation
        {
            return Err(PlatformError::new(
                ErrorCode::BindingCapabilityUnsupported,
                "binding is not a supported Vectorize capability",
            ));
        }
        Ok(binding)
    }

    async fn execute_json(
        &self,
        binding: AuthorizedBinding,
        pin: ResourcePin,
        call: JsonCall,
    ) -> Result<Response, PlatformError> {
        let operation = call.operation.clone();
        let write = operation == "deleteByIds";
        if write && !binding.binding.permissions.write
            || !write && !binding.binding.permissions.read
        {
            return Err(permission_denied());
        }
        let storage = self.storage.clone();
        let is_query = matches!(operation.as_str(), "query" | "queryById");
        let execute = move || {
            let _pin = pin;
            let engine = open_engine(&storage, &binding)?;
            let resource_id = binding.resource.id.to_string();
            match operation.as_str() {
                "describe" => {
                    require_empty_object(&call.payload)?;
                    let description = engine.describe()?;
                    Ok(json!({
                        "vectorCount": description.vector_count,
                        "dimensions": description.dimensions,
                        "processedUpToDatetime": description.processed_at_ms.unwrap_or(0),
                        "processedUpToMutation": description.processed_sequence,
                    }))
                }
                "getByIds" => {
                    let ids = parse_ids(&call.payload)?;
                    Ok(serde_json::to_value(engine.get_by_ids(&ids)?)
                        .map_err(|_| protocol_error())?)
                }
                "deleteByIds" => {
                    let ids = parse_ids(&call.payload)?;
                    let items = ids
                        .into_iter()
                        .map(|id| VectorMutationInput {
                            id,
                            namespace: None,
                            values: None,
                            metadata: None,
                        })
                        .collect::<Vec<_>>();
                    let receipt = engine.enqueue(VectorMutationKind::Delete, &items, unix_ms())?;
                    Ok(json!({"mutationId": receipt.mutation_id}))
                }
                "query" => {
                    let payload: QueryPayload =
                        serde_json::from_value(call.payload).map_err(|_| protocol_error())?;
                    execute_query(
                        &engine,
                        &resource_id,
                        &payload.vector,
                        &payload.options,
                        Instant::now() + QUERY_DEADLINE,
                    )
                }
                "queryById" => {
                    let payload: QueryByIdPayload =
                        serde_json::from_value(call.payload).map_err(|_| protocol_error())?;
                    let records = engine.get_by_ids(std::slice::from_ref(&payload.vector_id))?;
                    let query = records.first().ok_or_else(not_found)?;
                    execute_query(
                        &engine,
                        &resource_id,
                        &query.values,
                        &payload.options,
                        Instant::now() + QUERY_DEADLINE,
                    )
                }
                _ => Err(protocol_error()),
            }
        };
        let result = if is_query {
            run_query_cpu(execute).await?
        } else {
            tokio::task::spawn_blocking(execute)
                .await
                .map_err(|_| unavailable())??
        };
        json_response(&json!({"schemaVersion": 1, "result": result}))
    }

    async fn enqueue(
        &self,
        binding: AuthorizedBinding,
        pin: ResourcePin,
        kind: VectorMutationKind,
        items: Vec<VectorMutationInput>,
    ) -> Result<Response, PlatformError> {
        let storage = self.storage.clone();
        let task = tokio::task::spawn_blocking(move || {
            let _pin = pin;
            let engine = open_engine(&storage, &binding)?;
            let receipt = engine.enqueue(kind, &items, unix_ms())?;
            Ok::<_, PlatformError>(receipt.mutation_id)
        });
        let mutation_id = task.await.map_err(|_| unavailable())??;
        json_response(&json!({"schemaVersion": 1, "result": {"mutationId": mutation_id}}))
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct JsonCall {
    operation: String,
    payload: Value,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct QueryPayload {
    vector: Vec<f32>,
    #[serde(default)]
    options: QueryOptions,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct QueryByIdPayload {
    vector_id: String,
    #[serde(default)]
    options: QueryOptions,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct QueryOptions {
    #[serde(default = "default_top_k")]
    top_k: usize,
    namespace: Option<String>,
    #[serde(default)]
    return_values: bool,
    #[serde(default)]
    return_metadata: ReturnMetadata,
    filter: Option<Value>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum ReturnMetadata {
    #[default]
    None,
    Indexed,
    All,
}

impl<'de> Deserialize<'de> for ReturnMetadata {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;
        match value {
            Value::Bool(false) => Ok(Self::None),
            Value::Bool(true) => Ok(Self::All),
            Value::String(value) if value == "none" => Ok(Self::None),
            Value::String(value) if value == "all" => Ok(Self::All),
            Value::String(value) if value == "indexed" => Ok(Self::Indexed),
            _ => Err(serde::de::Error::custom("invalid metadata projection")),
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct QueryMatch {
    id: String,
    score: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    values: Option<Vec<f32>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    namespace: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    metadata: Option<Value>,
}

fn execute_query(
    engine: &VectorizeEngine,
    resource_id: &str,
    query: &[f32],
    options: &QueryOptions,
    deadline: Instant,
) -> Result<Value, PlatformError> {
    let maximum = if options.return_values || options.return_metadata == ReturnMetadata::All {
        MAX_TOP_K_WITH_VALUES
    } else {
        MAX_TOP_K
    };
    if options.top_k == 0 || options.top_k > maximum {
        return Err(limit_error());
    }
    let description = engine.describe()?;
    let metric = open_compute_search::DistanceMetric::from_str(&description.metric)
        .map_err(|_| corrupt())?;
    let indexed = engine.indexed_properties()?;
    let filter = options
        .filter
        .as_ref()
        .map(|filter| compile_filter(filter, &indexed).map_err(|_| protocol_error()))
        .transpose()?;
    let mut accumulator =
        ExactTopK::new(metric, query, options.top_k).map_err(|_| protocol_error())?;
    let warm_snapshot = if filter.is_none() {
        load_warm_snapshot(engine, resource_id)?
    } else {
        None
    };
    let (scores, records) = if let Some(snapshot) = &warm_snapshot {
        for record in &snapshot.records {
            if Instant::now() >= deadline {
                return Err(query_timeout());
            }
            if options
                .namespace
                .as_deref()
                .is_some_and(|namespace| record.namespace.as_deref() != Some(namespace))
            {
                continue;
            }
            accumulator
                .push(ExactCandidate {
                    id: &record.id,
                    values: &record.values,
                })
                .map_err(|_| corrupt())?;
        }
        let scores = accumulator.finish();
        let records = scores
            .iter()
            .filter_map(|scored| {
                snapshot
                    .records
                    .iter()
                    .find(|record| record.id == scored.id)
            })
            .cloned()
            .collect();
        (scores, records)
    } else {
        engine.with_read_snapshot(|snapshot| {
            snapshot.scan_candidates(options.namespace.as_deref(), filter.as_ref(), |record| {
                if Instant::now() >= deadline {
                    return Err(query_timeout());
                }
                if let Some(filter) = &filter {
                    let metadata = record.metadata.clone().unwrap_or_else(|| json!({}));
                    let metadata = validate_metadata(&metadata).map_err(|_| corrupt())?;
                    if !filter.matches(&metadata) {
                        return Ok(());
                    }
                }
                accumulator
                    .push(ExactCandidate {
                        id: &record.id,
                        values: &record.values,
                    })
                    .map_err(|_| corrupt())
            })?;
            let scores = accumulator.finish();
            let selected_ids = scores
                .iter()
                .map(|scored| scored.id.clone())
                .collect::<Vec<_>>();
            let records = snapshot.get_by_ids(&selected_ids)?;
            Ok((scores, records))
        })?
    };
    let matches = scores
        .into_iter()
        .map(|scored| {
            let record = records
                .iter()
                .find(|record| record.id == scored.id)
                .ok_or_else(corrupt)?;
            Ok(QueryMatch {
                id: scored.id,
                score: scored.score,
                values: options.return_values.then(|| record.values.clone()),
                namespace: record.namespace.clone(),
                metadata: project_metadata(record, options.return_metadata, &indexed),
            })
        })
        .collect::<Result<Vec<_>, PlatformError>>()?;
    serde_json::to_value(json!({"count": matches.len(), "matches": matches}))
        .map_err(|_| protocol_error())
}

async fn run_query_cpu<T: Send + 'static>(
    operation: impl FnOnce() -> Result<T, PlatformError> + Send + 'static,
) -> Result<T, PlatformError> {
    let pool = VECTOR_QUERY_POOL
        .get_or_init(|| {
            rayon::ThreadPoolBuilder::new()
                .num_threads(QUERY_THREADS)
                .thread_name(|index| format!("oc-vector-query-{index}"))
                .build()
                .ok()
        })
        .as_ref()
        .ok_or_else(unavailable)?;
    let semaphore = VECTOR_QUERY_SEMAPHORE
        .get_or_init(|| Arc::new(tokio::sync::Semaphore::new(QUERY_ADMISSION)))
        .clone();
    let permit = tokio::time::timeout(QUERY_DEADLINE, semaphore.acquire_owned())
        .await
        .map_err(|_| query_timeout())?
        .map_err(|_| unavailable())?;
    let (sender, receiver) = tokio::sync::oneshot::channel();
    pool.spawn(move || {
        let _permit = permit;
        let _ = sender.send(operation());
    });
    tokio::time::timeout(QUERY_DEADLINE, receiver)
        .await
        .map_err(|_| query_timeout())?
        .map_err(|_| unavailable())?
}

fn project_metadata(
    record: &VectorRecord,
    projection: ReturnMetadata,
    indexed: &std::collections::BTreeSet<String>,
) -> Option<Value> {
    match projection {
        ReturnMetadata::None => None,
        ReturnMetadata::All => record.metadata.clone(),
        ReturnMetadata::Indexed => {
            let source = record.metadata.as_ref()?;
            let mut output = serde_json::Map::new();
            for path in indexed {
                if let Some(value) = resolve_path(source, path) {
                    output.insert(path.clone(), truncate_indexed(value));
                }
            }
            Some(Value::Object(output))
        }
    }
}

fn resolve_path<'a>(value: &'a Value, path: &str) -> Option<&'a Value> {
    path.split('.').try_fold(value, |current, component| {
        current.as_object()?.get(component)
    })
}

fn truncate_indexed(value: &Value) -> Value {
    match value {
        Value::String(value) => Value::String(prefix_64(value)),
        Value::Array(values) => Value::Array(
            values
                .iter()
                .filter_map(Value::as_str)
                .map(|value| Value::String(prefix_64(value)))
                .collect(),
        ),
        value => value.clone(),
    }
}

fn prefix_64(value: &str) -> String {
    let mut end = value.len().min(64);
    while !value.is_char_boundary(end) {
        end = end.saturating_sub(1);
    }
    value[..end].to_string()
}

fn open_engine(
    storage: &PlatformStorage,
    binding: &AuthorizedBinding,
) -> Result<VectorizeEngine, PlatformError> {
    let record =
        VectorizeIndexRepository::new(storage.db()).get(binding.account_id, binding.resource.id)?;
    let path = VectorizePaths::open(storage.data_dir().root())?.resolve_storage_key(
        &record.storage_key,
        binding.account_id,
        binding.resource.id,
    )?;
    VectorizeEngine::open(
        &path,
        &binding.resource.id.to_string(),
        record.dimensions,
        &record.metric,
        record.quota_vectors,
        record.quota_bytes,
        storage.sqlite_busy_timeout_ms(),
    )
}

fn decode_mutation_frame(
    bytes: &[u8],
    expected: VectorMutationKind,
) -> Result<Vec<VectorMutationInput>, PlatformError> {
    let mut cursor = Cursor { bytes, offset: 0 };
    if cursor.take(4)? != b"OCVZ" || cursor.u16()? != 1 {
        return Err(protocol_error());
    }
    let header_len = usize::try_from(cursor.u32()?).map_err(|_| protocol_error())?;
    let header: FrameHeader =
        serde_json::from_slice(cursor.take(header_len)?).map_err(|_| protocol_error())?;
    let expected_operation = match expected {
        VectorMutationKind::Insert => "insert",
        VectorMutationKind::Upsert => "upsert",
        VectorMutationKind::Delete => "delete",
    };
    if header.schema_version != 1 || header.operation != expected_operation {
        return Err(protocol_error());
    }
    let count = usize::try_from(cursor.u32()?).map_err(|_| protocol_error())?;
    if count == 0 || count > 1_000 {
        return Err(limit_error());
    }
    let mut ids = std::collections::BTreeSet::new();
    let mut items = Vec::with_capacity(count);
    for _ in 0..count {
        let id_len = usize::from(cursor.u16()?);
        let id = cursor.string(id_len)?;
        let first_occurrence = ids.insert(id.clone());
        let namespace_len = cursor.u16()?;
        let namespace = if namespace_len == u16::MAX {
            None
        } else {
            Some(cursor.string(usize::from(namespace_len))?)
        };
        let metadata_len = cursor.u32()?;
        let metadata = if metadata_len == u32::MAX {
            None
        } else {
            let length = usize::try_from(metadata_len).map_err(|_| protocol_error())?;
            Some(serde_json::from_slice(cursor.take(length)?).map_err(|_| protocol_error())?)
        };
        let dimensions = usize::from(cursor.u16()?);
        let values = (0..dimensions)
            .map(|_| cursor.f32())
            .collect::<Result<Vec<_>, _>>()?;
        if first_occurrence {
            items.push(VectorMutationInput {
                id,
                namespace,
                values: Some(values),
                metadata,
            });
        }
    }
    if cursor.offset != bytes.len() {
        return Err(protocol_error());
    }
    Ok(items)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FrameHeader {
    operation: String,
    schema_version: u32,
}

struct Cursor<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Cursor<'a> {
    fn take(&mut self, length: usize) -> Result<&'a [u8], PlatformError> {
        let end = self.offset.checked_add(length).ok_or_else(protocol_error)?;
        let value = self
            .bytes
            .get(self.offset..end)
            .ok_or_else(protocol_error)?;
        self.offset = end;
        Ok(value)
    }
    fn u16(&mut self) -> Result<u16, PlatformError> {
        Ok(u16::from_be_bytes(
            self.take(2)?.try_into().map_err(|_| protocol_error())?,
        ))
    }
    fn u32(&mut self) -> Result<u32, PlatformError> {
        Ok(u32::from_be_bytes(
            self.take(4)?.try_into().map_err(|_| protocol_error())?,
        ))
    }
    fn f32(&mut self) -> Result<f32, PlatformError> {
        let value = f32::from_le_bytes(self.take(4)?.try_into().map_err(|_| protocol_error())?);
        if !value.is_finite() {
            return Err(protocol_error());
        }
        Ok(value)
    }
    fn string(&mut self, length: usize) -> Result<String, PlatformError> {
        std::str::from_utf8(self.take(length)?)
            .map(str::to_string)
            .map_err(|_| protocol_error())
    }
}

fn parse_ids(payload: &Value) -> Result<Vec<String>, PlatformError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Ids {
        ids: Vec<String>,
    }
    let value: Ids = serde_json::from_value(payload.clone()).map_err(|_| protocol_error())?;
    if value.ids.is_empty() || value.ids.len() > 1_000 {
        return Err(limit_error());
    }
    Ok(value.ids)
}

fn require_empty_object(value: &Value) -> Result<(), PlatformError> {
    if value.as_object().is_some_and(serde_json::Map::is_empty) {
        Ok(())
    } else {
        Err(protocol_error())
    }
}

fn parse_header<T: FromStr>(headers: &HeaderMap, name: &str) -> Result<T, PlatformError> {
    header_text(headers, name)?
        .parse()
        .map_err(|_| protocol_error())
}

fn header_text<'a>(headers: &'a HeaderMap, name: &str) -> Result<&'a str, PlatformError> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty())
        .ok_or_else(protocol_error)
}

fn parse_digest(headers: &HeaderMap, name: &str) -> Result<[u8; 32], PlatformError> {
    let bytes = hex::decode(header_text(headers, name)?).map_err(|_| protocol_error())?;
    bytes.try_into().map_err(|_| protocol_error())
}

fn content_type_is(headers: &HeaderMap, expected: &str) -> bool {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        == Some(expected)
}

fn json_response(value: &impl Serialize) -> Result<Response, PlatformError> {
    let bytes = serde_json::to_vec(value).map_err(|_| protocol_error())?;
    Ok(([(header::CONTENT_TYPE, "application/json")], bytes).into_response())
}

fn error_response(error: &PlatformError) -> Response {
    let status = match error.code() {
        ErrorCode::BindingPermissionDenied => StatusCode::FORBIDDEN,
        ErrorCode::ResourceNotFound => StatusCode::NOT_FOUND,
        ErrorCode::BindingLimitExceeded | ErrorCode::ResourceLimitExceeded => {
            StatusCode::PAYLOAD_TOO_LARGE
        }
        ErrorCode::ResourceUnavailable | ErrorCode::ResourceInvariantViolation => {
            StatusCode::SERVICE_UNAVAILABLE
        }
        _ => StatusCode::BAD_REQUEST,
    };
    let body = serde_json::to_vec(
        &json!({"error": {"code": error.code().as_str(), "message": error.message()}}),
    )
    .unwrap_or_default();
    (
        status,
        [
            ("x-open-compute-error-code", error.code().as_str()),
            (header::CONTENT_TYPE.as_str(), "application/json"),
        ],
        body,
    )
        .into_response()
}

fn default_top_k() -> usize {
    5
}
fn unix_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|value| i64::try_from(value.as_millis()).ok())
        .unwrap_or(0)
}
fn protocol_error() -> PlatformError {
    PlatformError::new(
        ErrorCode::BindingProtocolError,
        "Vectorize private request is invalid",
    )
}
fn permission_denied() -> PlatformError {
    PlatformError::new(
        ErrorCode::BindingPermissionDenied,
        "Vectorize binding permission denied",
    )
}
fn limit_error() -> PlatformError {
    PlatformError::new(
        ErrorCode::BindingLimitExceeded,
        "Vectorize request exceeds a fixed limit",
    )
}
fn unavailable() -> PlatformError {
    PlatformError::new(
        ErrorCode::ResourceUnavailable,
        "Vectorize execution is unavailable",
    )
}
fn corrupt() -> PlatformError {
    PlatformError::new(
        ErrorCode::ResourceInvariantViolation,
        "Vectorize authority is corrupt",
    )
}
fn not_found() -> PlatformError {
    PlatformError::new(
        ErrorCode::ResourceNotFound,
        "Vectorize vector was not found",
    )
}
fn query_timeout() -> PlatformError {
    PlatformError::new(
        ErrorCode::ResourceLimitExceeded,
        "Vectorize query exceeded its fixed CPU deadline",
    )
}

#[cfg(test)]
#[path = "vectorize_backend_tests.rs"]
mod tests;
