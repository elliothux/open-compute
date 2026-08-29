//! Structured KV transport, bounded staging, and streaming responses.

use super::{
    BackendState, KvBindingExecutor, backend_error, parse_json, platform_error, protocol_error,
};
use crate::kv_backend::{
    KvCommand, KvCommandResult, KvStagedValue, KvStagingLease, KvStreamPart,
    ensure_storage_headroom,
};
use crate::metrics::{BindingBackendOperation, KvOperation, KvStagingGauge, MetricsRegistry};
use axum::body::{Body, to_bytes};
use axum::extract::Request;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::Response;
use bytes::Bytes;
use futures::{StreamExt as _, TryStreamExt as _};
use open_compute_core::{BindingId, ErrorCode, PlatformError, ResourceId};
use open_compute_storage::{AuthorizedBinding, PlatformStorage};
use open_compute_workers::ResourcePin;
use serde::Deserialize;
use std::str::FromStr;
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;
use tokio::io::AsyncWriteExt as _;

pub(super) const FRAME_CONTENT_TYPE: &str = "application/vnd.open-compute.kv.v1+frame";
pub(super) const MAX_FRAME_BODY_BYTES: usize = open_compute_storage::KV_MAX_VALUE_BYTES + 64 * 1024;

#[derive(Clone)]
pub(super) struct StreamBudget {
    global: Arc<tokio::sync::Semaphore>,
    per_resource: usize,
    resources: Arc<Mutex<std::collections::HashMap<ResourceId, Weak<tokio::sync::Semaphore>>>>,
}

impl StreamBudget {
    pub(super) fn new(global: u32, per_resource: u32) -> Self {
        Self {
            global: Arc::new(tokio::sync::Semaphore::new(global.max(1) as usize)),
            per_resource: per_resource.max(1) as usize,
            resources: Arc::new(Mutex::new(std::collections::HashMap::new())),
        }
    }

    async fn acquire(
        &self,
        resource: ResourceId,
        timeout: Duration,
    ) -> Result<KvStagingLease, PlatformError> {
        let global = tokio::time::timeout(timeout, self.global.clone().acquire_owned())
            .await
            .map_err(|_| kv_busy())?
            .map_err(|_| kv_busy())?;
        let resource_gate = {
            let mut resources = self
                .resources
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            resources.retain(|_, gate| gate.strong_count() > 0);
            if let Some(gate) = resources.get(&resource).and_then(Weak::upgrade) {
                gate
            } else {
                let gate = Arc::new(tokio::sync::Semaphore::new(self.per_resource));
                resources.insert(resource, Arc::downgrade(&gate));
                gate
            }
        };
        let resource = tokio::time::timeout(timeout, resource_gate.acquire_owned())
            .await
            .map_err(|_| kv_busy())?
            .map_err(|_| kv_busy())?;
        Ok(KvStagingLease::new(global, resource))
    }
}

#[derive(Clone, Copy)]
pub(super) enum Operation {
    Get,
    GetWithMetadata,
    GetMany,
    Put,
    Delete,
    List,
}

impl Operation {
    pub(super) const fn metric(self) -> BindingBackendOperation {
        match self {
            Self::Get | Self::GetWithMetadata | Self::GetMany | Self::List => {
                BindingBackendOperation::Get
            }
            Self::Put => BindingBackendOperation::Put,
            Self::Delete => BindingBackendOperation::Delete,
        }
    }

    pub(super) const fn kv_metric(self) -> KvOperation {
        match self {
            Self::Get => KvOperation::Get,
            Self::GetWithMetadata => KvOperation::GetWithMetadata,
            Self::GetMany => KvOperation::GetMany,
            Self::Put => KvOperation::Put,
            Self::Delete => KvOperation::Delete,
            Self::List => KvOperation::List,
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct KeyRequest {
    key: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FrameGetRequest {
    keys: Vec<String>,
    cache_ttl: Option<u64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FramePutHeader {
    key: String,
    expiration: Option<u64>,
    expiration_ttl: Option<u64>,
    metadata: Option<serde_json::Value>,
    #[serde(default)]
    metadata_present: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FrameListRequest {
    #[serde(default)]
    prefix: String,
    limit: u16,
    cursor: Option<String>,
}

pub(super) async fn dispatch(
    state: BackendState,
    binding: AuthorizedBinding,
    operation: Operation,
    request_id: String,
    request: Request,
    pin: ResourcePin,
) -> Response {
    let command = if matches!(operation, Operation::Put) {
        match stage_put_frame(
            &state.storage,
            &binding,
            &request_id,
            request.into_body(),
            &state.stream_budget,
            state.executor.operation_timeout(),
            state.metrics.as_ref(),
        )
        .await
        {
            Ok(command) => command,
            Err(error) => {
                drop(pin);
                return platform_error(&error);
            }
        }
    } else {
        let Ok(bytes) = to_bytes(request.into_body(), MAX_FRAME_BODY_BYTES).await else {
            drop(pin);
            return backend_error(ErrorCode::KvValueTooLarge, StatusCode::PAYLOAD_TOO_LARGE);
        };
        match parse_frame_command(operation, &bytes) {
            Ok(command) => command,
            Err(error) => {
                drop(pin);
                return platform_error(&error);
            }
        }
    };
    if matches!(operation, Operation::Get | Operation::GetWithMetadata) {
        let KvCommand::Get {
            mut keys,
            cache_ttl,
        } = command
        else {
            drop(pin);
            return backend_error(ErrorCode::KvInternalProtocolError, StatusCode::BAD_REQUEST);
        };
        let Some(key) = keys.pop() else {
            drop(pin);
            return backend_error(ErrorCode::KvTooManyKeys, StatusCode::BAD_REQUEST);
        };
        return dispatch_stream_get(state.executor, binding, key, cache_ttl, pin).await;
    }
    let executor = state.executor.clone();
    let timeout = executor.operation_timeout();
    let blocking = tokio::task::spawn_blocking(move || {
        let _pin = pin;
        executor.execute(&binding, command)
    });
    let result = match tokio::time::timeout(timeout, blocking).await {
        Ok(Ok(result)) => result,
        Ok(Err(_)) => Err(protocol_error()),
        Err(_) if matches!(operation, Operation::Put | Operation::Delete) => Err(
            PlatformError::new(ErrorCode::KvResultUnknown, "KV mutation result is unknown"),
        ),
        Err(_) => Err(PlatformError::new(
            ErrorCode::KvUnavailable,
            "KV namespace operation timed out",
        )),
    };
    match result.and_then(|result| encode_frame_result(operation, result)) {
        Ok((content_type, bytes)) => {
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
        Err(error) => platform_error(&error),
    }
}

enum StreamMessage {
    Part(KvStreamPart),
    Complete,
    Error(PlatformError),
}

async fn dispatch_stream_get(
    executor: Arc<dyn KvBindingExecutor>,
    binding: AuthorizedBinding,
    key: String,
    cache_ttl: Option<u64>,
    pin: ResourcePin,
) -> Response {
    let timeout = executor.operation_timeout();
    let (sender, mut receiver) = tokio::sync::mpsc::channel(2);
    tokio::task::spawn_blocking(move || {
        let _pin = pin;
        let mut sink = |part| {
            sender
                .blocking_send(StreamMessage::Part(part))
                .map_err(|_| {
                    PlatformError::new(
                        ErrorCode::BindingProtocolError,
                        "KV response consumer cancelled the value stream",
                    )
                })
        };
        let terminal = match executor.stream_get(&binding, &key, cache_ttl, &mut sink) {
            Ok(()) => StreamMessage::Complete,
            Err(error) => StreamMessage::Error(error),
        };
        let _ = sender.blocking_send(terminal);
    });

    let first = match tokio::time::timeout(timeout, receiver.recv()).await {
        Ok(Some(StreamMessage::Part(KvStreamPart::Entry(entry)))) => entry,
        Ok(Some(StreamMessage::Error(error))) => return platform_error(&error),
        Ok(Some(StreamMessage::Part(KvStreamPart::Bytes(_)) | StreamMessage::Complete))
        | Ok(None) => return platform_error(&protocol_error()),
        Err(_) => {
            return platform_error(&PlatformError::new(
                ErrorCode::KvUnavailable,
                "KV namespace operation timed out",
            ));
        }
    };
    let value_length = first.as_ref().map_or(0, |entry| entry.value_length);
    let prefix = match encode_stream_header(first) {
        Ok(prefix) => prefix,
        Err(error) => return platform_error(&error),
    };
    let content_length = prefix.len().saturating_add(value_length);
    let deadline = tokio::time::Instant::now() + timeout;
    let tail = futures::stream::unfold(
        Some((receiver, deadline, value_length)),
        |state| async move {
            let (mut receiver, deadline, remaining) = state?;
            let Ok(message) = tokio::time::timeout_at(deadline, receiver.recv()).await else {
                return Some((
                    Err(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        ErrorCode::KvUnavailable.as_str(),
                    )),
                    None,
                ));
            };
            match message {
                Some(StreamMessage::Part(KvStreamPart::Bytes(bytes)))
                    if bytes.len() <= remaining =>
                {
                    let remaining = remaining - bytes.len();
                    Some((
                        Ok::<_, std::io::Error>(Bytes::from(bytes)),
                        Some((receiver, deadline, remaining)),
                    ))
                }
                Some(StreamMessage::Error(error)) => {
                    Some((Err(std::io::Error::other(error.code().as_str())), None))
                }
                Some(StreamMessage::Complete) if remaining == 0 => None,
                _ => Some((
                    Err(std::io::Error::other(
                        ErrorCode::KvInternalProtocolError.as_str(),
                    )),
                    None,
                )),
            }
        },
    );
    let body = futures::stream::once(async move { Ok::<_, std::io::Error>(Bytes::from(prefix)) })
        .chain(tail);
    let mut response = Response::new(Body::from_stream(body));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(FRAME_CONTENT_TYPE),
    );
    if let Ok(value) = HeaderValue::from_str(&content_length.to_string()) {
        response.headers_mut().insert(header::CONTENT_LENGTH, value);
    }
    response
}

fn encode_stream_header(
    entry: Option<open_compute_storage::KvEntryInfo>,
) -> Result<Vec<u8>, PlatformError> {
    let mut output = b"KVS1".to_vec();
    let Some(entry) = entry else {
        output.push(0);
        output.extend_from_slice(&(-1_i64).to_be_bytes());
        output.extend_from_slice(&u32::MAX.to_be_bytes());
        output.extend_from_slice(&u32::MAX.to_be_bytes());
        return Ok(output);
    };
    if entry.value_length > open_compute_storage::KV_MAX_VALUE_BYTES {
        return Err(PlatformError::new(
            ErrorCode::KvValueTooLarge,
            "KV value exceeds its byte limit",
        ));
    }
    output.push(1);
    output.extend_from_slice(&entry.expires_at_ms.unwrap_or(-1).to_be_bytes());
    if let Some(metadata) = entry.metadata_json {
        if metadata.len() > open_compute_storage::KV_MAX_METADATA_BYTES {
            return Err(PlatformError::new(
                ErrorCode::KvMetadataTooLarge,
                "KV metadata exceeds its byte limit",
            ));
        }
        let length = u32::try_from(metadata.len()).map_err(|_| kv_protocol_error())?;
        output.extend_from_slice(&length.to_be_bytes());
        output.extend_from_slice(&metadata);
    } else {
        output.extend_from_slice(&u32::MAX.to_be_bytes());
    }
    let value_length = u32::try_from(entry.value_length).map_err(|_| kv_protocol_error())?;
    output.extend_from_slice(&value_length.to_be_bytes());
    Ok(output)
}

async fn stage_put_frame(
    storage: &PlatformStorage,
    binding: &AuthorizedBinding,
    request_id: &str,
    body: Body,
    stream_budget: &StreamBudget,
    timeout: Duration,
    metrics: Option<&Arc<MetricsRegistry>>,
) -> Result<KvCommand, PlatformError> {
    let stream_permits = stream_budget.acquire(binding.resource.id, timeout).await?;
    let mut staging_metric = KvStagingGauge::new(metrics);
    let mut stream = body.into_data_stream();
    let mut header_bytes = Vec::with_capacity(4100);
    let mut header_end: Option<usize> = None;
    let mut header: Option<FramePutHeader> = None;
    let mut staged: Option<(std::path::PathBuf, tokio::fs::File)> = None;
    let mut value_length = 0_usize;

    loop {
        let chunk = match stream.try_next().await {
            Ok(Some(chunk)) => chunk,
            Ok(None) => break,
            Err(_) => {
                cleanup_staged(&mut staged).await;
                return Err(kv_protocol_error());
            }
        };
        let mut remaining = chunk.as_ref();
        while !remaining.is_empty() {
            if header.is_none() {
                let needed = match header_end {
                    Some(end) => end.saturating_sub(header_bytes.len()),
                    None => 4_usize.saturating_sub(header_bytes.len()),
                };
                let take = needed.min(remaining.len());
                header_bytes.extend_from_slice(&remaining[..take]);
                remaining = &remaining[take..];
                if header_end.is_none() && header_bytes.len() == 4 {
                    let length = usize::try_from(u32::from_be_bytes(
                        header_bytes[..4]
                            .try_into()
                            .map_err(|_| kv_protocol_error())?,
                    ))
                    .map_err(|_| kv_protocol_error())?;
                    if length > 4096 {
                        return Err(kv_protocol_error());
                    }
                    header_end = Some(4_usize.checked_add(length).ok_or_else(kv_protocol_error)?);
                }
                if header_end.is_some_and(|end| header_bytes.len() == end) {
                    let parsed = parse_json::<FramePutHeader>(&header_bytes[4..])?;
                    open_compute_storage::validate_key(&parsed.key)?;
                    let paths = open_compute_storage::KvPaths::open(storage.data_dir().root())?;
                    let path = paths.create_write_staging(binding.resource.id, request_id)?;
                    let Ok(file) = tokio::fs::OpenOptions::new()
                        .read(true)
                        .write(true)
                        .open(&path)
                        .await
                    else {
                        let _ = tokio::fs::remove_file(&path).await;
                        return Err(PlatformError::new(
                            ErrorCode::KvUnavailable,
                            "KV value staging file is unavailable",
                        ));
                    };
                    staged = Some((path, file));
                    header = Some(parsed);
                }
                continue;
            }

            value_length = value_length
                .checked_add(remaining.len())
                .ok_or_else(value_too_large)?;
            if value_length > open_compute_storage::KV_MAX_VALUE_BYTES {
                cleanup_staged(&mut staged).await;
                return Err(value_too_large());
            }
            let staged_bytes = remaining.len();
            if let Err(error) = ensure_storage_headroom(storage, remaining.len()) {
                cleanup_staged(&mut staged).await;
                return Err(error);
            }
            if let Some((_, file)) = staged.as_mut()
                && file.write_all(remaining).await.is_err()
            {
                cleanup_staged(&mut staged).await;
                return Err(PlatformError::new(
                    ErrorCode::KvStorageFull,
                    "failed to stage KV value bytes",
                ));
            }
            staging_metric.add(staged_bytes);
            remaining = &[];
        }
    }

    let Some(header) = header else {
        return Err(kv_protocol_error());
    };
    let Some((path, file)) = staged else {
        return Err(kv_protocol_error());
    };
    if file.sync_all().await.is_err() {
        let _ = tokio::fs::remove_file(&path).await;
        return Err(PlatformError::new(
            ErrorCode::KvStorageFull,
            "failed to sync KV value staging file",
        ));
    }
    let mut file = file.into_std().await;
    std::io::Seek::rewind(&mut file).map_err(|_| {
        PlatformError::new(
            ErrorCode::KvUnavailable,
            "KV value staging file is unavailable",
        )
    })?;
    Ok(KvCommand::PutStaged {
        key: header.key,
        value: KvStagedValue::with_lease(path, file, value_length, stream_permits)
            .with_staging_metric(staging_metric),
        expiration: header.expiration,
        expiration_ttl: header.expiration_ttl,
        metadata: header.metadata,
        metadata_present: header.metadata_present,
    })
}

async fn cleanup_staged(staged: &mut Option<(std::path::PathBuf, tokio::fs::File)>) {
    if let Some((path, file)) = staged.take() {
        drop(file);
        let _ = tokio::fs::remove_file(&path).await;
        if let Some(parent) = path.parent() {
            let _ = tokio::fs::remove_dir(parent).await;
        }
    }
}

fn value_too_large() -> PlatformError {
    PlatformError::new(
        ErrorCode::KvValueTooLarge,
        "KV value exceeds the 25 MiB limit",
    )
}

fn kv_busy() -> PlatformError {
    PlatformError::new(
        ErrorCode::KvBusy,
        "KV active stream limit is temporarily saturated",
    )
}

fn parse_frame_command(operation: Operation, bytes: &[u8]) -> Result<KvCommand, PlatformError> {
    match operation {
        Operation::Get | Operation::GetWithMetadata | Operation::GetMany => {
            let request = parse_json::<FrameGetRequest>(bytes)?;
            let valid_count = match operation {
                Operation::Get | Operation::GetWithMetadata => request.keys.len() == 1,
                Operation::GetMany => {
                    request.keys.len() <= open_compute_storage::KV_MAX_MULTI_GET_KEYS
                }
                _ => false,
            };
            if !valid_count {
                return Err(PlatformError::new(
                    ErrorCode::KvTooManyKeys,
                    "KV get key count is outside the supported range",
                ));
            }
            for key in &request.keys {
                open_compute_storage::validate_key(key)?;
            }
            Ok(KvCommand::Get {
                keys: request.keys,
                cache_ttl: request.cache_ttl,
            })
        }
        Operation::Put => {
            if bytes.len() < 4 {
                return Err(kv_protocol_error());
            }
            let header_len = usize::try_from(u32::from_be_bytes(
                bytes[..4].try_into().map_err(|_| kv_protocol_error())?,
            ))
            .map_err(|_| kv_protocol_error())?;
            let header_end = 4_usize
                .checked_add(header_len)
                .ok_or_else(kv_protocol_error)?;
            if header_len > 4096 || header_end > bytes.len() {
                return Err(kv_protocol_error());
            }
            let header = parse_json::<FramePutHeader>(&bytes[4..header_end])?;
            open_compute_storage::validate_key(&header.key)?;
            let value = bytes[header_end..].to_vec();
            if value.len() > open_compute_storage::KV_MAX_VALUE_BYTES {
                return Err(PlatformError::new(
                    ErrorCode::KvValueTooLarge,
                    "KV value exceeds the 25 MiB limit",
                ));
            }
            Ok(KvCommand::Put {
                key: header.key,
                value,
                expiration: header.expiration,
                expiration_ttl: header.expiration_ttl,
                metadata: header.metadata,
                metadata_present: header.metadata_present,
            })
        }
        Operation::Delete => {
            let request = parse_json::<KeyRequest>(bytes)?;
            open_compute_storage::validate_key(&request.key)?;
            Ok(KvCommand::Delete { key: request.key })
        }
        Operation::List => {
            let request = parse_json::<FrameListRequest>(bytes)?;
            Ok(KvCommand::List {
                prefix: request.prefix,
                limit: request.limit,
                cursor: request.cursor,
            })
        }
    }
}

fn encode_frame_result(
    operation: Operation,
    result: KvCommandResult,
) -> Result<(&'static str, Vec<u8>), PlatformError> {
    match (operation, result) {
        (Operation::Get | Operation::GetWithMetadata, KvCommandResult::Entries(mut entries))
            if entries.len() == 1 =>
        {
            let mut bytes = b"KVS1".to_vec();
            encode_entry(&mut bytes, entries.pop().unwrap_or(None))?;
            Ok((FRAME_CONTENT_TYPE, bytes))
        }
        (Operation::GetMany, KvCommandResult::Entries(entries)) => {
            let count = u16::try_from(entries.len()).map_err(|_| kv_protocol_error())?;
            let mut bytes = b"KVB1".to_vec();
            bytes.extend_from_slice(&count.to_be_bytes());
            for entry in entries {
                encode_entry(&mut bytes, entry)?;
            }
            Ok((FRAME_CONTENT_TYPE, bytes))
        }
        (Operation::Put | Operation::Delete, KvCommandResult::Mutation) => {
            Ok((FRAME_CONTENT_TYPE, Vec::new()))
        }
        (
            Operation::List,
            KvCommandResult::List {
                rows,
                complete,
                cursor,
            },
        ) => {
            let keys = rows
                .into_iter()
                .map(|row| {
                    let name = String::from_utf8(row.key).map_err(|_| {
                        PlatformError::new(ErrorCode::KvCorrupt, "KV key is not valid UTF-8")
                    })?;
                    let metadata = row
                        .metadata_json
                        .map(|bytes| {
                            serde_json::from_slice::<serde_json::Value>(&bytes).map_err(|_| {
                                PlatformError::new(
                                    ErrorCode::KvCorrupt,
                                    "KV metadata is not canonical JSON",
                                )
                            })
                        })
                        .transpose()?;
                    Ok(serde_json::json!({
                        "name": name,
                        "expiration": row.expires_at_ms.map(|value| value / 1000),
                        "metadata": metadata,
                    }))
                })
                .collect::<Result<Vec<_>, PlatformError>>()?;
            let bytes = serde_json::to_vec(&serde_json::json!({
                "keys": keys,
                "list_complete": complete,
                "cursor": cursor,
            }))
            .map_err(|_| kv_protocol_error())?;
            Ok(("application/json", bytes))
        }
        _ => Err(kv_protocol_error()),
    }
}

fn encode_entry(
    output: &mut Vec<u8>,
    entry: Option<open_compute_storage::KvEntry>,
) -> Result<(), PlatformError> {
    let Some(entry) = entry else {
        output.push(0);
        output.extend_from_slice(&(-1_i64).to_be_bytes());
        output.extend_from_slice(&u32::MAX.to_be_bytes());
        output.extend_from_slice(&u32::MAX.to_be_bytes());
        return Ok(());
    };
    output.push(1);
    output.extend_from_slice(&entry.expires_at_ms.unwrap_or(-1).to_be_bytes());
    if let Some(metadata) = entry.metadata_json {
        let length = u32::try_from(metadata.len()).map_err(|_| kv_protocol_error())?;
        output.extend_from_slice(&length.to_be_bytes());
        output.extend_from_slice(&metadata);
    } else {
        output.extend_from_slice(&u32::MAX.to_be_bytes());
    }
    let length = u32::try_from(entry.value.len()).map_err(|_| kv_protocol_error())?;
    output.extend_from_slice(&length.to_be_bytes());
    output.extend_from_slice(&entry.value);
    Ok(())
}

pub(super) fn parse_path(path: &str) -> Option<(BindingId, Operation)> {
    let rest = path.strip_prefix("/internal/bindings/v1/kv/")?;
    let (id, operation) = rest.split_once('/')?;
    if operation.contains('/') {
        return None;
    }
    let operation = match operation {
        "get" => Operation::Get,
        "get-with-metadata" => Operation::GetWithMetadata,
        "get-many" => Operation::GetMany,
        "put" => Operation::Put,
        "delete" => Operation::Delete,
        "list" => Operation::List,
        _ => return None,
    };
    Some((BindingId::from_str(id).ok()?, operation))
}

pub(super) fn declared_too_large(headers: &HeaderMap) -> bool {
    headers
        .get(header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<usize>().ok())
        .is_some_and(|length| length > MAX_FRAME_BODY_BYTES)
}

pub(super) fn permission_allows(binding: &AuthorizedBinding, operation: Operation) -> bool {
    match operation {
        Operation::Get | Operation::GetWithMetadata | Operation::GetMany | Operation::List => {
            binding.binding.permissions.read
        }
        Operation::Put | Operation::Delete => binding.binding.permissions.write,
    }
}

fn kv_protocol_error() -> PlatformError {
    PlatformError::new(
        ErrorCode::KvInternalProtocolError,
        "KV private protocol frame is invalid",
    )
}

#[cfg(test)]
#[path = "kv_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "kv_protocol_tests.rs"]
mod protocol_tests;
