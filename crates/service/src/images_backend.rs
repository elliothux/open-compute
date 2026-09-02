//! Authenticated bounded Images sessions executed by the one native raster engine.

mod options;

use crate::metrics::{ImageMetricOperation, ImageMetricOutcome, MetricsRegistry};
use axum::body::{Body, to_bytes};
use axum::extract::Request;
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use http_body_util::BodyExt as _;
use open_compute_core::{
    AccountId, AdmissionReservation, ErrorCode, ImagesConfig, PlatformError, VersionId, WorkerId,
};
use open_compute_images::{ImageEngine, ImageJob, ImageOperation, OutputOptions};
use open_compute_storage::{
    BuiltinBindingKind, PlatformStorage, VersionState, WorkerRepository, version_runtime_features,
};
use options::{DrawRequest, TransformRequest};
use serde::Serialize;
use std::collections::HashMap;
use std::fs::OpenOptions;
use std::os::unix::fs::OpenOptionsExt as _;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::AsyncWriteExt as _;
use tokio::sync::Semaphore;
use uuid::Uuid;

const ACCOUNT_HEADER: &str = "x-open-compute-account-id";
const WORKER_HEADER: &str = "x-open-compute-worker-id";
const VERSION_HEADER: &str = "x-open-compute-version-id";
const DESCRIPTOR_HEADER: &str = "x-open-compute-descriptor-sha256";
const GENERATION_HEADER: &str = "x-open-compute-startup-generation";
const ERROR_HEADER: &str = "x-open-compute-error-code";
const MAX_OPTIONS_BYTES: usize = 64 * 1024;

/// Sanitized operator capacity snapshot; no session or content identities are exposed.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageCapacity {
    /// Currently retained sessions.
    pub active_sessions: u64,
    /// Configured session ceiling.
    pub max_sessions: u16,
    /// Bytes retained by session staging files.
    pub retained_bytes: u64,
    /// Configured staging-byte ceiling.
    pub max_temp_bytes: u64,
    /// Native transforms currently holding global admission.
    pub active_transforms: u64,
    /// Configured process-wide transform concurrency.
    pub max_concurrency: u16,
}

#[derive(Debug)]
struct ImageSession {
    owner: ImageAuthority,
    base: PathBuf,
    retained_bytes: u64,
    overlays: Vec<PathBuf>,
    operations: Vec<ImageOperation>,
    _reservations: Vec<AdmissionReservation>,
    last_used: Instant,
}

impl Drop for ImageSession {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.base);
        for path in &self.overlays {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// Process-local image session owner; no image bytes enter control authority or backups.
pub struct ImageBindingService {
    storage: Arc<PlatformStorage>,
    engine: ImageEngine,
    config: ImagesConfig,
    sessions: Mutex<HashMap<String, ImageSession>>,
    global: Arc<Semaphore>,
    accounts: Mutex<HashMap<AccountId, Arc<Semaphore>>>,
    metrics: Option<Arc<MetricsRegistry>>,
}

impl std::fmt::Debug for ImageBindingService {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ImageBindingService")
            .finish_non_exhaustive()
    }
}

impl ImageBindingService {
    /// Compose the single native engine and bounded concurrency policy.
    #[must_use]
    pub fn new(storage: Arc<PlatformStorage>, config: ImagesConfig) -> Self {
        Self {
            storage,
            engine: ImageEngine::new(config.clone()),
            global: Arc::new(Semaphore::new(config.max_concurrency as usize)),
            accounts: Mutex::new(HashMap::new()),
            sessions: Mutex::new(HashMap::new()),
            metrics: None,
            config,
        }
    }

    /// Attach the process-wide fixed-series metrics registry.
    #[must_use]
    pub fn with_metrics(mut self, metrics: Arc<MetricsRegistry>) -> Self {
        self.metrics = Some(metrics);
        self
    }

    /// Inspect bounded process capacity after pruning expired sessions.
    pub fn capacity(&self) -> Result<ImageCapacity, PlatformError> {
        let mut sessions = self.sessions.lock().map_err(|_| unavailable())?;
        prune_sessions(&mut sessions, self.config.session_ttl_ms, None);
        Ok(ImageCapacity {
            active_sessions: sessions.len() as u64,
            max_sessions: self.config.max_sessions,
            retained_bytes: retained_bytes(&sessions),
            max_temp_bytes: self.config.max_temp_bytes,
            active_transforms: u64::try_from(
                usize::from(self.config.max_concurrency)
                    .saturating_sub(self.global.available_permits()),
            )
            .unwrap_or(u64::MAX),
            max_concurrency: self.config.max_concurrency,
        })
    }

    /// Drop every in-flight session when the supervised workerd generation exits.
    pub fn clear_sessions(&self) -> Result<(), PlatformError> {
        self.sessions.lock().map_err(|_| unavailable())?.clear();
        if let Some(metrics) = &self.metrics {
            metrics.set_image_active_sessions(0);
        }
        Ok(())
    }

    /// Dispatch one generation-authenticated private Images operation.
    pub async fn handle(&self, request: Request) -> Response {
        let operation = image_metric_operation(request.uri().path());
        let result = self.handle_result(request).await;
        if let (Some(metrics), Some(operation)) = (&self.metrics, operation) {
            let outcome = match &result {
                Ok(_) => ImageMetricOutcome::Success,
                Err(error) if error.code() == ErrorCode::ImageLimitExceeded => {
                    ImageMetricOutcome::Limit
                }
                Err(_) => ImageMetricOutcome::Failure,
            };
            metrics.observe_image(operation, outcome);
            let sessions = self
                .sessions
                .lock()
                .map_or(0, |sessions| sessions.len() as u64);
            metrics.set_image_active_sessions(sessions);
        }
        match result {
            Ok(response) => response,
            Err(error) => image_error(&error),
        }
    }

    async fn handle_result(&self, request: Request) -> Result<Response, PlatformError> {
        let authority = self.authorize(request.headers())?;
        let path = request.uri().path().to_owned();
        match path.as_str() {
            "/internal/images/v1/input" => self.input(authority, request).await,
            "/internal/images/v1/info" => self.info(authority, request).await,
            _ => {
                let (session, operation) = parse_session_path(&path)?;
                match operation {
                    "transform" => self.transform(authority, session, request).await,
                    "draw" => self.draw(authority, session, request).await,
                    "output" => self.output(authority, session, request).await,
                    _ => Err(protocol()),
                }
            }
        }
    }

    fn authorize(&self, headers: &HeaderMap) -> Result<ImageAuthority, PlatformError> {
        let account = parse_header::<AccountId>(headers, ACCOUNT_HEADER)?;
        let worker = parse_header::<WorkerId>(headers, WORKER_HEADER)?;
        let version = parse_header::<VersionId>(headers, VERSION_HEADER)?;
        let digest = hex::decode(text_header(headers, DESCRIPTOR_HEADER)?)
            .ok()
            .and_then(|value| <[u8; 32]>::try_from(value).ok())
            .ok_or_else(protocol)?;
        let record =
            WorkerRepository::new(self.storage.db()).get_version(account, worker, version)?;
        if record.state != VersionState::Ready || record.deleted_at_ms.is_some() {
            return Err(protocol());
        }
        let (_, bindings) = version_runtime_features(self.storage.db(), version)?;
        if !bindings.iter().any(|binding| {
            binding.kind == BuiltinBindingKind::Images && binding.descriptor_sha256 == digest
        }) {
            return Err(protocol());
        }
        Ok(ImageAuthority {
            account,
            worker,
            version,
            descriptor: text_header(headers, DESCRIPTOR_HEADER)?.to_owned(),
            generation: text_header(headers, GENERATION_HEADER)?.to_owned(),
        })
    }

    async fn input(
        &self,
        owner: ImageAuthority,
        request: Request,
    ) -> Result<Response, PlatformError> {
        let admission = self
            .storage
            .reserve_mutation(self.config.max_input_bytes)
            .map_err(|_| limit())?;
        let mut staged = stage_body(
            request.into_body(),
            self.storage.data_dir().version_staging_dir(),
            self.config.max_input_bytes,
            "image-input",
        )
        .await?;
        let bytes =
            std::fs::read(staged.path.as_ref().ok_or_else(protocol)?).map_err(|_| unavailable())?;
        self.engine.info(&bytes)?;
        if let Some(metrics) = &self.metrics {
            metrics.add_image_bytes(true, staged.size);
        }
        let session = Uuid::now_v7().to_string();
        let mut sessions = self.sessions.lock().map_err(|_| unavailable())?;
        prune_sessions(
            &mut sessions,
            self.config.session_ttl_ms,
            Some(&owner.generation),
        );
        if sessions.len() >= usize::from(self.config.max_sessions)
            || retained_bytes(&sessions).saturating_add(staged.size) > self.config.max_temp_bytes
        {
            return Err(limit());
        }
        sessions.insert(
            session.clone(),
            ImageSession {
                owner,
                base: staged.path.take().ok_or_else(protocol)?,
                retained_bytes: staged.size,
                overlays: Vec::new(),
                operations: Vec::new(),
                _reservations: vec![admission],
                last_used: Instant::now(),
            },
        );
        Ok(axum::Json(serde_json::json!({ "session": session })).into_response())
    }

    async fn info(
        &self,
        _owner: ImageAuthority,
        request: Request,
    ) -> Result<Response, PlatformError> {
        let _admission = self
            .storage
            .reserve_mutation(self.config.max_input_bytes)
            .map_err(|_| limit())?;
        let staged = stage_body(
            request.into_body(),
            self.storage.data_dir().version_staging_dir(),
            self.config.max_input_bytes,
            "image-info",
        )
        .await?;
        let bytes =
            std::fs::read(staged.path.as_ref().ok_or_else(protocol)?).map_err(|_| unavailable())?;
        let info = self.engine.info(&bytes)?;
        if let Some(metrics) = &self.metrics {
            metrics.add_image_bytes(true, staged.size);
        }
        Ok(axum::Json(info).into_response())
    }

    async fn transform(
        &self,
        owner: ImageAuthority,
        session: String,
        request: Request,
    ) -> Result<Response, PlatformError> {
        let bytes = to_bytes(request.into_body(), MAX_OPTIONS_BYTES)
            .await
            .map_err(|_| option())?;
        let options: TransformRequest = serde_json::from_slice(&bytes).map_err(|_| option())?;
        let operations = options.operations()?;
        let mut sessions = self.sessions.lock().map_err(|_| unavailable())?;
        prune_sessions(
            &mut sessions,
            self.config.session_ttl_ms,
            Some(&owner.generation),
        );
        let value = sessions.get_mut(&session).ok_or_else(protocol)?;
        value.require_owner(&owner)?;
        if value.operations.len().saturating_add(operations.len())
            > usize::from(self.config.max_operations)
        {
            return Err(limit());
        }
        value.operations.extend(operations);
        value.last_used = Instant::now();
        Ok(StatusCode::NO_CONTENT.into_response())
    }

    async fn draw(
        &self,
        owner: ImageAuthority,
        session: String,
        request: Request,
    ) -> Result<Response, PlatformError> {
        {
            let sessions = self.sessions.lock().map_err(|_| unavailable())?;
            sessions
                .get(&session)
                .ok_or_else(protocol)?
                .require_owner(&owner)?;
        }
        let admission = self
            .storage
            .reserve_mutation(self.config.max_input_bytes)
            .map_err(|_| limit())?;
        let mut staged = stage_framed(
            request.into_body(),
            self.storage.data_dir().version_staging_dir(),
            self.config.max_input_bytes,
            "image-overlay",
        )
        .await?;
        let options: DrawRequest =
            serde_json::from_slice(&staged.metadata).map_err(|_| option())?;
        options.validate()?;
        let bytes =
            std::fs::read(staged.path.as_ref().ok_or_else(protocol)?).map_err(|_| unavailable())?;
        self.engine.info(&bytes)?;
        if let Some(metrics) = &self.metrics {
            metrics.add_image_bytes(true, staged.size);
        }
        let mut sessions = self.sessions.lock().map_err(|_| unavailable())?;
        prune_sessions(
            &mut sessions,
            self.config.session_ttl_ms,
            Some(&owner.generation),
        );
        let total = retained_bytes(&sessions).saturating_add(staged.size);
        let value = sessions.get_mut(&session).ok_or_else(protocol)?;
        value.require_owner(&owner)?;
        if value.overlays.len() >= usize::from(self.config.max_overlays)
            || value.operations.len() >= usize::from(self.config.max_operations)
            || total > self.config.max_temp_bytes
        {
            return Err(limit());
        }
        let overlay = u16::try_from(value.overlays.len()).map_err(|_| limit())?;
        value
            .overlays
            .push(staged.path.take().ok_or_else(protocol)?);
        value.retained_bytes = value.retained_bytes.saturating_add(staged.size);
        value._reservations.push(admission);
        value.operations.push(ImageOperation::Draw {
            overlay,
            x: options.left,
            y: options.top,
            opacity: options.opacity,
        });
        value.last_used = Instant::now();
        Ok(StatusCode::NO_CONTENT.into_response())
    }

    async fn output(
        &self,
        owner: ImageAuthority,
        session: String,
        request: Request,
    ) -> Result<Response, PlatformError> {
        let bytes = to_bytes(request.into_body(), MAX_OPTIONS_BYTES)
            .await
            .map_err(|_| option())?;
        let output: OutputOptions = serde_json::from_slice(&bytes).map_err(|_| option())?;
        let session = {
            let mut sessions = self.sessions.lock().map_err(|_| unavailable())?;
            prune_sessions(
                &mut sessions,
                self.config.session_ttl_ms,
                Some(&owner.generation),
            );
            sessions
                .get(&session)
                .ok_or_else(protocol)?
                .require_owner(&owner)?;
            sessions.remove(&session).ok_or_else(protocol)?
        };
        let deadline =
            tokio::time::Instant::now() + Duration::from_millis(self.config.request_timeout_ms);
        let account_limit = {
            let mut accounts = self.accounts.lock().map_err(|_| unavailable())?;
            accounts.retain(|_, limit| Arc::strong_count(limit) > 1);
            accounts
                .entry(owner.account)
                .or_insert_with(|| {
                    Arc::new(Semaphore::new(
                        self.config.max_concurrency_per_account as usize,
                    ))
                })
                .clone()
        };
        let global = tokio::time::timeout_at(deadline, self.global.clone().acquire_owned())
            .await
            .map_err(|_| timeout())?
            .map_err(|_| unavailable())?;
        let account = tokio::time::timeout_at(deadline, account_limit.acquire_owned())
            .await
            .map_err(|_| timeout())?
            .map_err(|_| unavailable())?;
        let _transform_metrics = self.metrics.as_ref().map(MetricsRegistry::image_transform);
        let engine = self.engine.clone();
        let task = tokio::task::spawn_blocking(move || {
            let _global = global;
            let _account = account;
            let input = std::fs::read(&session.base).map_err(|_| unavailable())?;
            let overlays = session
                .overlays
                .iter()
                .map(|path| std::fs::read(path).map_err(|_| unavailable()))
                .collect::<Result<Vec<_>, _>>()?;
            engine.transform(&ImageJob {
                input,
                overlays,
                operations: session.operations.clone(),
                output,
            })
        });
        let transformed = tokio::time::timeout_at(deadline, task)
            .await
            .map_err(|_| timeout())?
            .map_err(|_| unavailable())??;
        if let Some(metrics) = &self.metrics {
            metrics.add_image_bytes(false, transformed.bytes.len() as u64);
        }
        Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, transformed.format.mime_type())
            .header(header::CONTENT_LENGTH, transformed.bytes.len())
            .body(Body::from(transformed.bytes))
            .map_err(|_| unavailable())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ImageAuthority {
    account: AccountId,
    worker: WorkerId,
    version: VersionId,
    descriptor: String,
    generation: String,
}

impl ImageSession {
    fn require_owner(&self, owner: &ImageAuthority) -> Result<(), PlatformError> {
        if &self.owner == owner {
            Ok(())
        } else {
            Err(protocol())
        }
    }
}

struct StagedFrame {
    path: Option<PathBuf>,
    metadata: Vec<u8>,
    size: u64,
}

impl Drop for StagedFrame {
    fn drop(&mut self) {
        if let Some(path) = &self.path {
            let _ = std::fs::remove_file(path);
        }
    }
}

struct StagedFile {
    path: Option<PathBuf>,
    size: u64,
}

impl Drop for StagedFile {
    fn drop(&mut self) {
        if let Some(path) = &self.path {
            let _ = std::fs::remove_file(path);
        }
    }
}

async fn stage_body(
    mut body: Body,
    directory: PathBuf,
    maximum: u64,
    suffix: &str,
) -> Result<StagedFile, PlatformError> {
    let path = directory.join(format!("{}.{suffix}", Uuid::now_v7()));
    let mut staged = StagedFile {
        path: Some(path.clone()),
        size: 0,
    };
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)
        .map_err(|_| unavailable())?;
    let mut file = tokio::fs::File::from_std(file);
    let mut size = 0_u64;
    while let Some(frame) = body.frame().await {
        let frame = frame.map_err(|_| invalid_input())?;
        let Ok(data) = frame.into_data() else {
            continue;
        };
        size = size
            .checked_add(u64::try_from(data.len()).map_err(|_| limit())?)
            .ok_or_else(limit)?;
        if size > maximum {
            return Err(limit());
        }
        file.write_all(&data).await.map_err(|_| unavailable())?;
    }
    file.sync_all().await.map_err(|_| unavailable())?;
    staged.size = size;
    Ok(staged)
}

async fn stage_framed(
    mut body: Body,
    directory: PathBuf,
    maximum: u64,
    suffix: &str,
) -> Result<StagedFrame, PlatformError> {
    let path = directory.join(format!("{}.{suffix}", Uuid::now_v7()));
    let mut staged = StagedFrame {
        path: Some(path.clone()),
        metadata: Vec::new(),
        size: 0,
    };
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)
        .map_err(|_| unavailable())?;
    let mut file = tokio::fs::File::from_std(file);
    let mut prefix = Vec::new();
    let mut metadata_length = None;
    while let Some(frame) = body.frame().await {
        let frame = frame.map_err(|_| invalid_input())?;
        let Ok(mut data) = frame.into_data() else {
            continue;
        };
        if metadata_length.is_none() {
            let needed = 4_usize.saturating_sub(prefix.len());
            let take = needed.min(data.len());
            prefix.extend_from_slice(&data.split_to(take));
            if prefix.len() == 4 {
                let length =
                    u32::from_be_bytes(prefix.as_slice().try_into().map_err(|_| protocol())?)
                        as usize;
                if length == 0 || length > MAX_OPTIONS_BYTES {
                    return Err(protocol());
                }
                metadata_length = Some(length);
            }
        }
        if let Some(length) = metadata_length
            && staged.metadata.len() < length
        {
            let take = (length - staged.metadata.len()).min(data.len());
            staged.metadata.extend_from_slice(&data.split_to(take));
        }
        if metadata_length.is_some_and(|length| staged.metadata.len() == length) && !data.is_empty()
        {
            staged.size = staged
                .size
                .checked_add(u64::try_from(data.len()).map_err(|_| limit())?)
                .ok_or_else(limit)?;
            if staged.size > maximum {
                return Err(limit());
            }
            file.write_all(&data).await.map_err(|_| unavailable())?;
        }
    }
    if metadata_length != Some(staged.metadata.len()) {
        return Err(protocol());
    }
    file.sync_all().await.map_err(|_| unavailable())?;
    Ok(staged)
}

fn parse_session_path(path: &str) -> Result<(String, &str), PlatformError> {
    let rest = path
        .strip_prefix("/internal/images/v1/session/")
        .ok_or_else(protocol)?;
    let (session, operation) = rest.split_once('/').ok_or_else(protocol)?;
    if Uuid::parse_str(session).is_err() || operation.contains('/') {
        return Err(protocol());
    }
    Ok((session.to_owned(), operation))
}

fn prune_sessions(
    sessions: &mut HashMap<String, ImageSession>,
    ttl_ms: u64,
    generation: Option<&str>,
) {
    let ttl = Duration::from_millis(ttl_ms);
    sessions.retain(|_, session| {
        session.last_used.elapsed() < ttl
            && generation.is_none_or(|generation| session.owner.generation == generation)
    });
}

fn retained_bytes(sessions: &HashMap<String, ImageSession>) -> u64 {
    sessions.values().fold(0_u64, |total, session| {
        total.saturating_add(session.retained_bytes)
    })
}

fn text_header<'a>(headers: &'a HeaderMap, name: &str) -> Result<&'a str, PlatformError> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty())
        .ok_or_else(protocol)
}
fn parse_header<T: FromStr>(headers: &HeaderMap, name: &str) -> Result<T, PlatformError> {
    text_header(headers, name)?.parse().map_err(|_| protocol())
}

fn image_error(error: &PlatformError) -> Response {
    let status = match error.code() {
        ErrorCode::ImageInputInvalid
        | ErrorCode::ImageOptionUnsupported
        | ErrorCode::ImageProtocolError => StatusCode::BAD_REQUEST,
        ErrorCode::ImageFormatUnsupported => StatusCode::UNSUPPORTED_MEDIA_TYPE,
        ErrorCode::ImageLimitExceeded => StatusCode::PAYLOAD_TOO_LARGE,
        ErrorCode::ImageTimeout => StatusCode::GATEWAY_TIMEOUT,
        _ => StatusCode::SERVICE_UNAVAILABLE,
    };
    let mut response = status.into_response();
    response.headers_mut().insert(
        HeaderName::from_static(ERROR_HEADER),
        HeaderValue::from_static(error.code().as_str()),
    );
    response
}

fn invalid_input() -> PlatformError {
    PlatformError::new(ErrorCode::ImageInputInvalid, "image input is invalid")
}
fn option() -> PlatformError {
    PlatformError::new(
        ErrorCode::ImageOptionUnsupported,
        "image option is unsupported",
    )
}
fn limit() -> PlatformError {
    PlatformError::new(ErrorCode::ImageLimitExceeded, "image limit was exceeded")
}
fn timeout() -> PlatformError {
    PlatformError::new(ErrorCode::ImageTimeout, "image execution timed out")
}
fn unavailable() -> PlatformError {
    PlatformError::new(ErrorCode::ImageUnavailable, "image engine is unavailable")
}
fn protocol() -> PlatformError {
    PlatformError::new(ErrorCode::ImageProtocolError, "image protocol is invalid")
}

fn image_metric_operation(path: &str) -> Option<ImageMetricOperation> {
    if path == "/internal/images/v1/input" {
        Some(ImageMetricOperation::Input)
    } else if path == "/internal/images/v1/info" {
        Some(ImageMetricOperation::Info)
    } else if path.ends_with("/transform") {
        Some(ImageMetricOperation::Transform)
    } else if path.ends_with("/draw") {
        Some(ImageMetricOperation::Draw)
    } else if path.ends_with("/output") {
        Some(ImageMetricOperation::Output)
    } else {
        None
    }
}

#[cfg(test)]
#[path = "images_backend_tests.rs"]
mod tests;
