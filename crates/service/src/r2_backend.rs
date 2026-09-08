//! Authorized private data plane for the loaded-isolate R2 facade.

#[path = "r2_backend_multipart.rs"]
pub(crate) mod multipart;
#[path = "r2_backend_objects.rs"]
pub(crate) mod objects;
#[path = "r2_backend_staging.rs"]
mod staging;

use crate::metrics::{
    MetricsRegistry, R2Operation, R2ProviderError, R2StreamDirection, R2StreamGuard,
};
use crate::r2_protocol::*;
use axum::body::Body;
use axum::http::{HeaderValue, Method, header};
use axum::response::Response;
use base64::Engine as _;
use bytes::Bytes;
use futures::StreamExt as _;
use open_compute_artifacts::{
    ObjectBody, R2GetResult, R2ObjectMetadata, R2ObjectStore, R2UploadSource, UserObjectKey,
    hash_file,
};
use open_compute_core::{
    AccountId, BindingKind, ErrorCode, OperationClass, PlatformError, R2Config, RequestId,
    ResourceId, VersionId,
};
use open_compute_storage::{
    AuthorizedBinding, BindingRepository, PlatformStorage, R2BucketRepository, R2ObjectListEntry,
    R2ObjectRecord, R2ObjectRepository, R2Staging,
};
use open_compute_workers::{ResourcePin, ResourcePins};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;
use tokio::io::AsyncWriteExt as _;

mod service;

pub use service::R2BindingService;

struct StagedPut {
    header: PutHeader,
    length: u64,
    checksums: open_compute_artifacts::R2ComputedChecksums,
    guard: StagingFile,
    _reservation: StagingReservation,
}

struct StagedPart {
    header: UploadPartHeader,
    length: u64,
    guard: StagingFile,
    _reservation: StagingReservation,
}

struct StagingFile {
    path: std::path::PathBuf,
}
impl StagingFile {
    fn new(path: std::path::PathBuf) -> Self {
        Self { path }
    }
}
impl Drop for StagingFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
        if let Some(parent) = self.path.parent() {
            let _ = std::fs::remove_dir(parent);
        }
    }
}

struct StagingReservation {
    used: Arc<AtomicU64>,
    max: u64,
    bytes: u64,
    metrics: Option<Arc<MetricsRegistry>>,
}
impl StagingReservation {
    fn new(used: Arc<AtomicU64>, max: u64, metrics: Option<Arc<MetricsRegistry>>) -> Self {
        Self {
            used,
            max,
            bytes: 0,
            metrics,
        }
    }
    fn add(&mut self, bytes: u64) -> Result<(), PlatformError> {
        let mut current = self.used.load(Ordering::Acquire);
        loop {
            let next = current.checked_add(bytes).ok_or_else(overloaded)?;
            if next > self.max {
                return Err(overloaded());
            }
            match self.used.compare_exchange_weak(
                current,
                next,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => {
                    self.bytes = self.bytes.saturating_add(bytes);
                    if let Some(metrics) = &self.metrics {
                        metrics.adjust_r2_staging_bytes(bytes, true);
                        metrics.add_r2_bytes(R2StreamDirection::Upload, bytes);
                    }
                    return Ok(());
                }
                Err(found) => current = found,
            }
        }
    }
}
impl Drop for StagingReservation {
    fn drop(&mut self) {
        self.used.fetch_sub(self.bytes, Ordering::AcqRel);
        if let Some(metrics) = &self.metrics {
            metrics.adjust_r2_staging_bytes(self.bytes, false);
        }
    }
}

#[derive(Clone)]
struct OperationGate {
    global: Arc<tokio::sync::Semaphore>,
    per_resource: usize,
    resources: Arc<Mutex<HashMap<ResourceId, Weak<tokio::sync::Semaphore>>>>,
}
impl OperationGate {
    fn new(limit: u32) -> Self {
        let limit = limit.max(1) as usize;
        Self {
            global: Arc::new(tokio::sync::Semaphore::new(limit)),
            per_resource: limit,
            resources: Arc::new(Mutex::new(HashMap::new())),
        }
    }
    async fn acquire(
        &self,
        resource: ResourceId,
        timeout: Duration,
    ) -> Result<OperationLease, PlatformError> {
        let global = tokio::time::timeout(timeout, self.global.clone().acquire_owned())
            .await
            .map_err(|_| overloaded())?
            .map_err(|_| overloaded())?;
        let gate = {
            let mut resources = self
                .resources
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            resources.retain(|_, gate| gate.strong_count() > 0);
            resources
                .get(&resource)
                .and_then(Weak::upgrade)
                .unwrap_or_else(|| {
                    let gate = Arc::new(tokio::sync::Semaphore::new(self.per_resource));
                    resources.insert(resource, Arc::downgrade(&gate));
                    gate
                })
        };
        let resource = tokio::time::timeout(timeout, gate.acquire_owned())
            .await
            .map_err(|_| overloaded())?
            .map_err(|_| overloaded())?;
        Ok(OperationLease {
            _global: global,
            _resource: resource,
        })
    }
}
struct OperationLease {
    _global: tokio::sync::OwnedSemaphorePermit,
    _resource: tokio::sync::OwnedSemaphorePermit,
}

fn framed_metadata(
    metadata: &R2ObjectMetadata,
    body: Option<ObjectBody>,
    pin: ResourcePin,
    lease: OperationLease,
    timeout: Duration,
    metrics: Option<&Arc<MetricsRegistry>>,
) -> Result<Response, PlatformError> {
    let has_body = body.is_some();
    let expected = metadata
        .range
        .and_then(|range| range.length)
        .unwrap_or(metadata.size);
    let header_bytes =
        serde_json::to_vec(&serde_json::json!({"meta": metadata, "hasBody": has_body}))
            .map_err(|_| protocol_error())?;
    if header_bytes.len() > MAX_METADATA_BYTES {
        return Err(metadata_too_large());
    }
    let mut prefix = u32::try_from(header_bytes.len())
        .map_err(|_| protocol_error())?
        .to_be_bytes()
        .to_vec();
    prefix.extend_from_slice(&header_bytes);
    let mut response = if let Some(body) = body {
        struct State {
            body: ObjectBody,
            remaining: u64,
            deadline: tokio::time::Instant,
            failed: bool,
            metrics: Option<Arc<MetricsRegistry>>,
            _stream: Option<R2StreamGuard>,
            _pin: ResourcePin,
            _lease: OperationLease,
        }
        let stream_metrics = metrics.cloned();
        let active = stream_metrics
            .as_ref()
            .map(|metrics| R2StreamGuard::new(metrics, R2StreamDirection::Download));
        let stream = futures::stream::unfold(
            State {
                body,
                remaining: expected,
                deadline: tokio::time::Instant::now() + timeout,
                failed: false,
                metrics: stream_metrics,
                _stream: active,
                _pin: pin,
                _lease: lease,
            },
            |mut state| async move {
                if state.failed {
                    return None;
                }
                match tokio::time::timeout_at(state.deadline, state.body.next()).await {
                    Ok(Some(Ok(bytes)))
                        if u64::try_from(bytes.len())
                            .ok()
                            .is_some_and(|size| size <= state.remaining) =>
                    {
                        let count = bytes.len() as u64;
                        state.remaining -= count;
                        if let Some(metrics) = &state.metrics {
                            metrics.add_r2_bytes(R2StreamDirection::Download, count);
                        }
                        Some((Ok::<Bytes, std::io::Error>(bytes), state))
                    }
                    Ok(None) if state.remaining == 0 => None,
                    _ => {
                        state.failed = true;
                        Some((
                            Err(std::io::Error::other(
                                ErrorCode::R2ProviderUnavailable.as_str(),
                            )),
                            state,
                        ))
                    }
                }
            },
        );
        let stream =
            futures::stream::once(async move { Ok::<Bytes, std::io::Error>(Bytes::from(prefix)) })
                .chain(stream);
        Response::new(Body::from_stream(stream))
    } else {
        drop(pin);
        drop(lease);
        Response::new(Body::from(prefix))
    };
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(FRAME_CONTENT_TYPE),
    );
    Ok(response)
}

async fn read_object_bytes(
    body: ObjectBody,
    expected_size: u64,
    timeout: Duration,
) -> Result<Vec<u8>, PlatformError> {
    let collected = tokio::time::timeout(timeout, body.collect())
        .await
        .map_err(|_| protocol_error())?
        .map_err(|_| protocol_error())?;
    let bytes = collected.into_bytes().to_vec();
    if u64::try_from(bytes.len()).map_err(|_| object_too_large())? != expected_size {
        return Err(metadata_invalid());
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests;
