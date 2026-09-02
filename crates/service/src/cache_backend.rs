//! Authenticated Worker response-cache authority over per-Worker SQLite and immutable S3 bodies.

use crate::metrics::{CacheMetricOperation, CacheS3Operation, MetricsRegistry};
use axum::body::{Body, to_bytes};
use axum::extract::Request;
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use bytes::Bytes;
use futures::stream;
use http_body_util::BodyExt as _;
use open_compute_artifacts::{ARTIFACT_KEY_VERSION, ArtifactCache, ArtifactRef, ArtifactStore};
use open_compute_core::{
    AccountId, DeploymentId, ErrorCode, PlatformError, ResponseCacheConfig, WorkerId,
};
use open_compute_storage::{
    CacheBodyRef, CacheIdentity, CacheLookupStatus, CacheManager, CacheMethod, CachePurge,
    CachePut, CacheStoredResponse, CacheSurface, DeploymentState, PlatformStorage,
    WorkerRepository, deployment_runtime_features,
};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::OpenOptions;
use std::os::unix::fs::OpenOptionsExt as _;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use uuid::Uuid;

const ACCOUNT_HEADER: &str = "x-open-compute-account-id";
const WORKER_HEADER: &str = "x-open-compute-worker-id";
const DEPLOYMENT_HEADER: &str = "x-open-compute-deployment-id";
const ENTRYPOINT_HEADER: &str = "x-open-compute-entrypoint";
const DESCRIPTOR_HEADER: &str = "x-open-compute-descriptor-sha256";
const ENABLED_HEADER: &str = "x-open-compute-cache-automatic-enabled";
const CROSS_VERSION_HEADER: &str = "x-open-compute-cache-cross-version";
const ERROR_HEADER: &str = "x-open-compute-error-code";
const MAX_METADATA_BYTES: usize = 64 * 1024;

mod policy;
use policy::{
    CachedResponsePlan, cache_deadlines, cached_response_plan, canonical_header_map,
    canonical_headers, comma_values, has_forbidden_cache_directive,
};

/// Composed cache product service shared by every loaded isolate.
#[derive(Clone)]
pub struct CacheBindingService {
    storage: Arc<PlatformStorage>,
    manager: Arc<CacheManager>,
    artifacts: ArtifactStore,
    artifact_cache: Arc<ArtifactCache>,
    config: ResponseCacheConfig,
    metrics: Option<Arc<MetricsRegistry>>,
}

impl std::fmt::Debug for CacheBindingService {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CacheBindingService")
            .field("manager", &self.manager)
            .finish_non_exhaustive()
    }
}

impl CacheBindingService {
    /// Compose the cache authority from the stable data root and immutable artifact store.
    pub fn new(
        storage: Arc<PlatformStorage>,
        artifacts: ArtifactStore,
        artifact_cache: Arc<ArtifactCache>,
        config: ResponseCacheConfig,
    ) -> Result<Self, PlatformError> {
        let manager = Arc::new(CacheManager::open(
            storage.data_dir().root(),
            config.clone(),
        )?);
        Ok(Self {
            storage,
            manager,
            artifacts,
            artifact_cache,
            config,
            metrics: None,
        })
    }

    /// Attach the process-wide fixed-series metrics registry.
    #[must_use]
    pub fn with_metrics(mut self, metrics: Arc<MetricsRegistry>) -> Self {
        self.metrics = Some(metrics);
        self
    }

    /// Share the metadata manager with GC and operator surfaces.
    #[must_use]
    pub fn manager(&self) -> Arc<CacheManager> {
        self.manager.clone()
    }

    /// Dispatch one generation-authenticated private cache operation.
    pub async fn handle(&self, request: Request) -> Response {
        let operation = cache_metric_operation(request.uri().path());
        let result = tokio::time::timeout(
            Duration::from_millis(self.config.request_timeout_ms),
            self.handle_result(request),
        )
        .await
        .unwrap_or_else(|_| Err(unavailable()));
        if let (Some(metrics), Some(operation)) = (&self.metrics, operation) {
            metrics.observe_response_cache(operation, result.is_ok());
        }
        match result {
            Ok(response) => response,
            Err(error) => cache_error(&error),
        }
    }

    async fn handle_result(&self, request: Request) -> Result<Response, PlatformError> {
        let authority = self.authorize(request.headers())?;
        match request.uri().path() {
            "/internal/cache/v1/match" => self.match_entry(authority, request).await,
            "/internal/cache/v1/put" => self.put_entry(authority, request).await,
            "/internal/cache/v1/delete" => self.delete_entry(authority, request).await,
            "/internal/cache/v1/purge" => self.purge(authority, request).await,
            _ => Err(protocol()),
        }
    }

    fn authorize(&self, headers: &HeaderMap) -> Result<CacheAuthority, PlatformError> {
        let account = parse_header::<AccountId>(headers, ACCOUNT_HEADER)?;
        let worker = parse_header::<WorkerId>(headers, WORKER_HEADER)?;
        let deployment = parse_header::<DeploymentId>(headers, DEPLOYMENT_HEADER)?;
        let entrypoint = text_header(headers, ENTRYPOINT_HEADER)?.to_owned();
        if !valid_entrypoint(&entrypoint) {
            return Err(protocol());
        }
        let descriptor = text_header(headers, DESCRIPTOR_HEADER)?;
        let descriptor: [u8; 32] = hex::decode(descriptor)
            .ok()
            .and_then(|value| value.try_into().ok())
            .ok_or_else(protocol)?;
        let automatic_enabled = bool_header(headers, ENABLED_HEADER)?;
        let cross_version_cache = bool_header(headers, CROSS_VERSION_HEADER)?;
        let record =
            WorkerRepository::new(self.storage.db()).get_deployment(account, worker, deployment)?;
        if record.state != DeploymentState::Ready
            || record.deleted_at_ms.is_some()
            || record.worker_code_sha256 != descriptor
        {
            return Err(protocol());
        }
        let (policies, _) = deployment_runtime_features(self.storage.db(), deployment)?;
        let selected = policies
            .iter()
            .find(|policy| {
                policy.entrypoint.as_deref()
                    == (entrypoint != "default").then_some(entrypoint.as_str())
            })
            .or_else(|| policies.iter().find(|policy| policy.entrypoint.is_none()))
            .ok_or_else(protocol)?;
        if selected.enabled != automatic_enabled
            || selected.cross_version_cache != cross_version_cache
        {
            return Err(protocol());
        }
        Ok(CacheAuthority {
            account,
            worker,
            deployment,
            entrypoint,
            automatic_enabled,
            cross_version_cache,
        })
    }

    async fn match_entry(
        &self,
        authority: CacheAuthority,
        request: Request,
    ) -> Result<Response, PlatformError> {
        let body = to_bytes(request.into_body(), MAX_METADATA_BYTES)
            .await
            .map_err(|_| protocol())?;
        let input: CacheRequest = serde_json::from_slice(&body).map_err(|_| protocol())?;
        let (identity, headers) = input.resolve(&authority, false)?;
        // Protect the selected remote body until the verified local file pin
        // owns the response stream. A concurrent purge may remove metadata,
        // but artifact GC cannot delete the object in this handoff window.
        let _artifact_lifecycle = self.artifacts.reserve_deployment_artifact().await;
        let engine = self
            .manager
            .engine(authority.account, authority.worker, now_ms())?;
        let lookup = engine.lookup(&identity, &headers, now_ms())?;
        if lookup.refresh_token.is_some()
            && let Some(metrics) = &self.metrics
        {
            metrics.observe_response_cache(CacheMetricOperation::Refresh, true);
        }
        let Some(ref stored) = lookup.response else {
            let mut response = StatusCode::NO_CONTENT.into_response();
            insert_lookup_headers(response.headers_mut(), &lookup)?;
            return Ok(response);
        };
        let mut response_headers = HeaderMap::new();
        for value in &stored.headers {
            response_headers.append(
                HeaderName::from_bytes(value.name.as_bytes()).map_err(|_| protocol())?,
                HeaderValue::from_str(&value.value).map_err(|_| protocol())?,
            );
        }
        response_headers.remove("cache-tag");
        response_headers.insert(
            HeaderName::from_static("x-open-compute-cache-hit"),
            HeaderValue::from_static("1"),
        );
        response_headers.insert(
            HeaderName::from_static("cf-cache-status"),
            HeaderValue::from_static(lookup_status(lookup.status)),
        );
        insert_lookup_headers(&mut response_headers, &lookup)?;
        let plan = cached_response_plan(stored, &headers, identity.method, &mut response_headers)?;
        if let CachedResponsePlan::Empty { status } = plan {
            response_headers.remove(header::CONTENT_LENGTH);
            if status == 304 {
                response_headers.remove(header::CONTENT_RANGE);
            }
            let mut response = Response::builder().status(status);
            *response.headers_mut().ok_or_else(protocol)? = response_headers;
            return response.body(Body::empty()).map_err(|_| protocol());
        }
        if identity.method == CacheMethod::Head {
            response_headers.insert(
                header::CONTENT_LENGTH,
                HeaderValue::from_str(&stored.body.size.to_string()).map_err(|_| protocol())?,
            );
            let mut response = Response::builder().status(stored.status);
            *response.headers_mut().ok_or_else(protocol)? = response_headers;
            return response.body(Body::empty()).map_err(|_| protocol());
        }
        let artifact =
            ArtifactRef::new(ARTIFACT_KEY_VERSION, &stored.body.sha256, stored.body.size)?;
        let started = Instant::now();
        let pinned = self
            .artifact_cache
            .acquire(&self.artifacts, &artifact)
            .await
            .map_err(|error| cache_artifact_error(&error))?;
        if let Some(metrics) = &self.metrics {
            metrics.observe_response_cache_s3(CacheS3Operation::Get, started.elapsed());
        }
        let reader = pinned.into_async_reader();
        let (status, skip, remaining) = match plan {
            CachedResponsePlan::Full => (stored.status, 0, stored.body.size),
            CachedResponsePlan::Range { start, length } => (206, start, length),
            CachedResponsePlan::Empty { .. } => unreachable!("empty cache response returned above"),
        };
        let body_stream = stream::try_unfold(
            (reader, skip, remaining),
            |(mut reader, mut skip, remaining)| async move {
                let mut discarded = vec![0_u8; 64 * 1024];
                while skip > 0 {
                    let requested = usize::try_from(skip.min(discarded.len() as u64))
                        .map_err(|_| std::io::Error::other("cache range overflow"))?;
                    let count = reader.read(&mut discarded[..requested]).await?;
                    if count == 0 {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::UnexpectedEof,
                            "cached body ended before requested range",
                        ));
                    }
                    skip = skip.saturating_sub(count as u64);
                }
                if remaining == 0 {
                    return Ok(None);
                }
                let requested = usize::try_from(remaining.min(64 * 1024))
                    .map_err(|_| std::io::Error::other("cache range overflow"))?;
                let mut bytes = vec![0_u8; requested];
                let count = reader.read(&mut bytes).await?;
                if count == 0 {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::UnexpectedEof,
                        "cached body ended before its verified size",
                    ));
                }
                bytes.truncate(count);
                Ok(Some((
                    Bytes::from(bytes),
                    (reader, 0, remaining.saturating_sub(count as u64)),
                )))
            },
        );
        let mut response = Response::builder().status(status);
        *response.headers_mut().ok_or_else(protocol)? = response_headers;
        response
            .body(Body::from_stream(body_stream))
            .map_err(|_| protocol())
    }

    async fn put_entry(
        &self,
        authority: CacheAuthority,
        request: Request,
    ) -> Result<Response, PlatformError> {
        let _admission = self
            .storage
            .reserve_mutation(self.config.max_object_bytes)
            .map_err(|_| limit())?;
        let staged = stage_framed_body(
            request.into_body(),
            self.storage.data_dir().deployment_staging_dir(),
            self.config.max_object_bytes,
        )
        .await?;
        let input: CachePutRequest =
            serde_json::from_slice(&staged.metadata).map_err(|_| protocol())?;
        let (identity, request_headers) = input.request.resolve(&authority, true)?;
        if identity.surface == CacheSurface::Automatic && !authority.automatic_enabled {
            return Err(put_rejected());
        }
        if input.status == 206 || !(200..=599).contains(&input.status) {
            return Err(put_rejected());
        }
        let response_headers = canonical_headers(input.response_headers)?;
        if request_headers.contains_key("authorization")
            || request_headers
                .get("cache-control")
                .is_some_and(|value| has_forbidden_cache_directive(value))
            || response_headers
                .iter()
                .any(|header| header.name == "set-cookie")
        {
            return Err(put_rejected());
        }
        let vary = comma_values(&response_headers, "vary")?;
        if vary.iter().any(|value| value == "*") {
            return Err(put_rejected());
        }
        let tags = comma_values(&response_headers, "cache-tag")?;
        let now = now_ms();
        let Some((fresh, swr, sie)) =
            cache_deadlines(&response_headers, now, identity.surface, input.status)?
        else {
            // Cloudflare's explicit Cache API resolves put() even when a status
            // without explicit freshness is not admitted. Automatic caching is
            // filtered before transport and never uses this silent path.
            return Ok(StatusCode::NO_CONTENT.into_response());
        };
        let engine = self
            .manager
            .engine(authority.account, authority.worker, now)?;
        let (current_fence, generation) = engine.prepare_put_generation(&identity)?;
        let (fence, refresh_token) = if identity.surface == CacheSurface::Automatic {
            let fence = input
                .expected_fence_generation
                .as_deref()
                .ok_or_else(protocol)?
                .parse::<u64>()
                .ok()
                .filter(|value| *value > 0)
                .ok_or_else(protocol)?;
            if input.refresh_token.as_deref().is_some_and(|token| {
                token.len() != 32 || !token.bytes().all(|byte| byte.is_ascii_hexdigit())
            }) {
                return Err(protocol());
            }
            (fence, input.refresh_token)
        } else {
            if input.expected_fence_generation.is_some() || input.refresh_token.is_some() {
                return Err(protocol());
            }
            (current_fence, None)
        };
        // Keep GC excluded from the first immutable-body observation through the
        // SQLite reference commit. Otherwise a concurrent final reference scan
        // can classify this body as an orphan between S3 PUT and metadata commit.
        let _artifact_lifecycle = self.artifacts.reserve_deployment_artifact().await;
        let started = Instant::now();
        let artifact = self
            .artifacts
            .put_verified_file(&staged.path, &staged.sha256, staged.size)
            .await
            .map_err(|error| cache_artifact_error(&error))?;
        if let Some(metrics) = &self.metrics {
            metrics.observe_response_cache_s3(CacheS3Operation::Put, started.elapsed());
        }
        let refresh = refresh_token.is_some();
        let result = engine.put(&CachePut {
            identity,
            request_headers,
            response: CacheStoredResponse {
                status: input.status,
                headers: response_headers
                    .into_iter()
                    .filter(|header| header.name != "cache-tag")
                    .collect(),
                body: CacheBodyRef {
                    sha256: artifact.sha256_hex(),
                    size: artifact.size(),
                },
                vary,
                tags,
                fresh_until_ms: fresh,
                stale_while_revalidate_until_ms: swr,
                stale_if_error_until_ms: sie,
                generation,
            },
            expected_fence_generation: fence,
            refresh_token,
            now_ms: now,
        });
        if refresh && let Some(metrics) = &self.metrics {
            metrics.observe_response_cache(CacheMetricOperation::Refresh, result.is_ok());
        }
        result?;
        Ok(StatusCode::NO_CONTENT.into_response())
    }

    async fn delete_entry(
        &self,
        authority: CacheAuthority,
        request: Request,
    ) -> Result<Response, PlatformError> {
        let body = to_bytes(request.into_body(), MAX_METADATA_BYTES)
            .await
            .map_err(|_| protocol())?;
        let input: CacheRequest = serde_json::from_slice(&body).map_err(|_| protocol())?;
        let (identity, headers) = input.resolve(&authority, false)?;
        let engine = self
            .manager
            .engine(authority.account, authority.worker, now_ms())?;
        let deleted = engine.delete(&identity, &headers)?;
        Ok(axum::Json(serde_json::json!({ "deleted": deleted })).into_response())
    }

    async fn purge(
        &self,
        authority: CacheAuthority,
        request: Request,
    ) -> Result<Response, PlatformError> {
        let body = to_bytes(request.into_body(), MAX_METADATA_BYTES)
            .await
            .map_err(|_| protocol())?;
        let purge: CachePurge = serde_json::from_slice(&body).map_err(|_| protocol())?;
        let now = now_ms();
        let engine = self
            .manager
            .engine(authority.account, authority.worker, now)?;
        let deleted = engine.purge(&purge, now)?;
        Ok(axum::Json(serde_json::json!({ "deleted": deleted, "success": true })).into_response())
    }
}

#[derive(Clone, Debug)]
struct CacheAuthority {
    account: AccountId,
    worker: WorkerId,
    deployment: DeploymentId,
    entrypoint: String,
    automatic_enabled: bool,
    cross_version_cache: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CacheRequest {
    namespace: String,
    name: Option<String>,
    url: String,
    method: String,
    headers: Vec<(String, String)>,
}

impl CacheRequest {
    fn resolve(
        self,
        authority: &CacheAuthority,
        put: bool,
    ) -> Result<(CacheIdentity, BTreeMap<String, String>), PlatformError> {
        let surface = match self.namespace.as_str() {
            "automatic" => CacheSurface::Automatic,
            "default" => CacheSurface::CacheApiDefault,
            "named" => CacheSurface::CacheApiNamed,
            _ => return Err(protocol()),
        };
        if (surface == CacheSurface::CacheApiNamed) != self.name.is_some() {
            return Err(protocol());
        }
        if put && self.method != "GET" {
            return Err(put_rejected());
        }
        let method = match self.method.as_str() {
            "GET" => CacheMethod::Get,
            "HEAD" if !put => CacheMethod::Head,
            _ => return Err(protocol()),
        };
        let mut url = url::Url::parse(&self.url).map_err(|_| protocol())?;
        if !matches!(url.scheme(), "http" | "https") || url.fragment().is_some() {
            return Err(protocol());
        }
        if matches!(
            (url.scheme(), url.port()),
            ("http", Some(80)) | ("https", Some(443))
        ) {
            url.set_port(None).map_err(|()| protocol())?;
        }
        if let Some(host) = url.host_str().map(str::to_ascii_lowercase) {
            url.set_host(Some(&host)).map_err(|_| protocol())?;
        }
        let headers = canonical_header_map(self.headers)?;
        Ok((
            CacheIdentity {
                account_id: authority.account,
                worker_id: authority.worker,
                surface,
                entrypoint: (surface == CacheSurface::Automatic)
                    .then(|| authority.entrypoint.clone()),
                version_scope: if surface == CacheSurface::Automatic
                    && !authority.cross_version_cache
                {
                    authority.deployment.to_string()
                } else {
                    "shared".to_owned()
                },
                cache_name: if surface == CacheSurface::CacheApiNamed {
                    Some(self.name.ok_or_else(protocol)?)
                } else {
                    None
                },
                canonical_url: url.into(),
                method,
            },
            headers,
        ))
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CachePutRequest {
    #[serde(flatten)]
    request: CacheRequest,
    status: u16,
    response_headers: Vec<(String, String)>,
    expected_fence_generation: Option<String>,
    refresh_token: Option<String>,
}

struct StagedBody {
    path: PathBuf,
    metadata: Vec<u8>,
    sha256: String,
    size: u64,
}

impl Drop for StagedBody {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

async fn stage_framed_body(
    mut body: Body,
    directory: PathBuf,
    maximum: u64,
) -> Result<StagedBody, PlatformError> {
    let path = directory.join(format!("{}.cache-upload", Uuid::now_v7()));
    let mut staged = StagedBody {
        path,
        metadata: Vec::new(),
        sha256: String::new(),
        size: 0,
    };
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&staged.path)
        .map_err(|_| {
            PlatformError::new(ErrorCode::CacheUnavailable, "cache staging is unavailable")
        })?;
    let mut file = tokio::fs::File::from_std(file);
    let mut prefix = Vec::new();
    let mut metadata_length = None;
    let mut size = 0_u64;
    let mut hasher = Sha256::new();
    while let Some(frame) = body.frame().await {
        let frame = frame.map_err(|_| protocol())?;
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
                if length == 0 || length > MAX_METADATA_BYTES {
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
            size = size
                .checked_add(u64::try_from(data.len()).map_err(|_| protocol())?)
                .ok_or_else(limit)?;
            if size > maximum {
                return Err(limit());
            }
            hasher.update(&data);
            file.write_all(&data).await.map_err(|_| unavailable())?;
        }
    }
    if metadata_length != Some(staged.metadata.len()) {
        return Err(protocol());
    }
    file.sync_all().await.map_err(|_| unavailable())?;
    drop(file);
    staged.sha256 = hex::encode(hasher.finalize());
    staged.size = size;
    Ok(staged)
}

fn lookup_status(status: CacheLookupStatus) -> &'static str {
    match status {
        CacheLookupStatus::Hit => "HIT",
        CacheLookupStatus::Miss => "MISS",
        CacheLookupStatus::Expired => "EXPIRED",
        CacheLookupStatus::Updating => "UPDATING",
        CacheLookupStatus::Stale => "STALE",
        CacheLookupStatus::StaleIfError => "STALE_IF_ERROR",
    }
}

fn insert_lookup_headers(
    headers: &mut HeaderMap,
    lookup: &open_compute_storage::CacheLookup,
) -> Result<(), PlatformError> {
    headers.insert(
        HeaderName::from_static("x-open-compute-cache-status"),
        HeaderValue::from_static(lookup_status(lookup.status)),
    );
    headers.insert(
        HeaderName::from_static("x-open-compute-cache-fence"),
        HeaderValue::from_str(&lookup.fence_generation.to_string()).map_err(|_| protocol())?,
    );
    if let Some(token) = &lookup.refresh_token {
        headers.insert(
            HeaderName::from_static("x-open-compute-cache-refresh-token"),
            HeaderValue::from_str(token).map_err(|_| protocol())?,
        );
    }
    Ok(())
}

fn add_seconds(value: i64, seconds: u64) -> Result<i64, PlatformError> {
    value
        .checked_add(i64::try_from(seconds.saturating_mul(1_000)).map_err(|_| limit())?)
        .ok_or_else(limit)
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

fn bool_header(headers: &HeaderMap, name: &str) -> Result<bool, PlatformError> {
    match text_header(headers, name)? {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err(protocol()),
    }
}

fn valid_entrypoint(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_alphabetic() || matches!(byte, b'_' | b'$'))
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'$'))
}

fn now_ms() -> i64 {
    i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
    )
    .unwrap_or(i64::MAX)
}

fn cache_error(error: &PlatformError) -> Response {
    let status = match error.code() {
        ErrorCode::CacheKeyInvalid | ErrorCode::CacheProtocolError => StatusCode::BAD_REQUEST,
        ErrorCode::CachePutRejected => StatusCode::UNPROCESSABLE_ENTITY,
        ErrorCode::CacheLimitExceeded => StatusCode::PAYLOAD_TOO_LARGE,
        ErrorCode::CacheUnavailable | ErrorCode::CacheResultUnknown => {
            StatusCode::SERVICE_UNAVAILABLE
        }
        ErrorCode::CacheCorrupt => StatusCode::INTERNAL_SERVER_ERROR,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    };
    let mut response = status.into_response();
    response.headers_mut().insert(
        HeaderName::from_static(ERROR_HEADER),
        HeaderValue::from_static(error.code().as_str()),
    );
    response
}

fn protocol() -> PlatformError {
    PlatformError::new(ErrorCode::CacheProtocolError, "cache protocol is invalid")
}
fn put_rejected() -> PlatformError {
    PlatformError::new(
        ErrorCode::CachePutRejected,
        "cache response cannot be stored",
    )
}
fn limit() -> PlatformError {
    PlatformError::new(ErrorCode::CacheLimitExceeded, "cache limit was exceeded")
}
fn unavailable() -> PlatformError {
    PlatformError::new(ErrorCode::CacheUnavailable, "cache storage is unavailable")
}

fn cache_artifact_error(error: &PlatformError) -> PlatformError {
    if matches!(
        error.code(),
        ErrorCode::ArtifactIntegrityError | ErrorCode::CacheEntryCorrupt | ErrorCode::PathInvalid
    ) {
        PlatformError::new(ErrorCode::CacheCorrupt, "cache body integrity check failed")
    } else {
        unavailable()
    }
}

fn cache_metric_operation(path: &str) -> Option<CacheMetricOperation> {
    match path {
        "/internal/cache/v1/match" => Some(CacheMetricOperation::Lookup),
        "/internal/cache/v1/put" => Some(CacheMetricOperation::Store),
        "/internal/cache/v1/delete" => Some(CacheMetricOperation::Delete),
        "/internal/cache/v1/purge" => Some(CacheMetricOperation::Purge),
        _ => None,
    }
}

#[cfg(test)]
#[path = "cache_backend_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "cache_backend_protocol_tests.rs"]
mod protocol_tests;
