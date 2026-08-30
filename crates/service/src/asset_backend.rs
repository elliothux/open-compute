//! Private deployment-scoped static-assets binding backend.

use axum::body::Body;
use axum::extract::Request;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::Response;
use bytes::Bytes;
use hyper::body::{Body as HttpBody, Frame, SizeHint};
use open_compute_artifacts::{ARTIFACT_KEY_VERSION, ArtifactCache, ArtifactRef, ArtifactStore};
use open_compute_core::{DeploymentId, ErrorCode, PlatformError};
use open_compute_storage::{
    DeploymentAssetsRepository, DeploymentRecord, PlatformStorage, WorkerRepository,
};
use open_compute_workers::{
    AssetManifestV1, AssetRequest, AssetResponsePlan, AssetRoutingConfigV1, DeploymentPin,
    DeploymentPins, plan_asset_response,
};
use std::pin::Pin;
use std::str::FromStr;
use std::sync::Arc;
use std::task::{Context, Poll};
use tokio::io::AsyncReadExt;
use url::Url;

const ASSET_METHOD_HEADER: &str = "x-open-compute-asset-method";
const ASSET_URL_HEADER: &str = "x-open-compute-asset-url";
const ASSET_REPRESENTATION_LENGTH_HEADER: &str = "x-open-compute-asset-representation-length";
const DEPLOYMENT_HEADER: &str = "x-open-compute-deployment-id";
const DESCRIPTOR_HEADER: &str = "x-open-compute-descriptor-sha256";
const ERROR_HEADER: &str = "x-open-compute-error-code";

/// Composed private asset planner, authority, verified cache, and body streamer.
#[derive(Clone, Debug)]
pub struct AssetBindingService {
    storage: Arc<PlatformStorage>,
    artifacts: ArtifactStore,
    cache: Arc<ArtifactCache>,
    pins: DeploymentPins,
}

impl AssetBindingService {
    /// Bind private static asset reads to platform authorities.
    #[must_use]
    pub fn new(
        storage: Arc<PlatformStorage>,
        artifacts: ArtifactStore,
        cache: Arc<ArtifactCache>,
        pins: DeploymentPins,
    ) -> Self {
        Self {
            storage,
            artifacts,
            cache,
            pins,
        }
    }

    /// Handle one generation-authenticated request after the shared backend token check.
    pub async fn handle(&self, request: Request) -> Response {
        match self.handle_authorized(request).await {
            Ok(response) => response,
            Err(error) => asset_error(&error),
        }
    }

    async fn handle_authorized(&self, request: Request) -> Result<Response, PlatformError> {
        let deployment_id = request
            .headers()
            .get(DEPLOYMENT_HEADER)
            .and_then(|value| value.to_str().ok())
            .ok_or_else(protocol_error)
            .and_then(|value| DeploymentId::from_str(value).map_err(|_| protocol_error()))?;
        let descriptor = request
            .headers()
            .get(DESCRIPTOR_HEADER)
            .and_then(|value| value.to_str().ok())
            .ok_or_else(protocol_error)
            .and_then(parse_digest)?;
        let method = request
            .headers()
            .get(ASSET_METHOD_HEADER)
            .and_then(|value| value.to_str().ok())
            .ok_or_else(protocol_error)?
            .to_owned();
        let url = request
            .headers()
            .get(ASSET_URL_HEADER)
            .and_then(|value| value.to_str().ok())
            .ok_or_else(protocol_error)
            .and_then(|value| Url::parse(value).map_err(|_| protocol_error()))?;
        if !matches!(url.scheme(), "http" | "https")
            || url.username() != ""
            || url.password().is_some()
        {
            return Err(protocol_error());
        }
        let host = url.host_str().ok_or_else(protocol_error)?;
        let sec_fetch_mode = request
            .headers()
            .get("sec-fetch-mode")
            .and_then(|value| value.to_str().ok());
        let if_none_match = request
            .headers()
            .get(header::IF_NONE_MATCH)
            .and_then(|value| value.to_str().ok());
        let has_authorization = request.headers().contains_key(header::AUTHORIZATION);
        let has_range = request.headers().contains_key(header::RANGE);
        let pin = self.pins.pin(deployment_id)?;
        let (account_id, worker_id, assets) = DeploymentAssetsRepository::new(self.storage.db())
            .authorize_ready(deployment_id, &descriptor)
            .map_err(|_| invariant())?;
        let manifest = serde_json::from_slice::<AssetManifestV1>(&assets.manifest_json)
            .map_err(|_| invariant())?;
        let routing = serde_json::from_slice::<AssetRoutingConfigV1>(&assets.routing_config_json)
            .map_err(|_| invariant())?;
        if manifest.sha256()? != assets.manifest_sha256
            || manifest.canonical_bytes()? != assets.manifest_json
            || routing.canonical_bytes()? != assets.routing_config_json
        {
            return Err(invariant());
        }
        let deployment = WorkerRepository::new(self.storage.db()).get_deployment(
            account_id,
            worker_id,
            deployment_id,
        )?;
        let plan = plan_asset_response(
            &manifest,
            &routing,
            AssetRequest {
                method: &method,
                path: url.path(),
                query: url.query(),
                host,
                sec_fetch_mode,
                if_none_match,
                has_authorization,
                has_range,
            },
        )?;
        let mut response = serve_asset_plan(
            &self.storage,
            &self.artifacts,
            Some(&self.cache),
            &deployment,
            plan,
        )
        .await?;
        if method == "HEAD"
            && let Some(length) = response.headers_mut().remove(header::CONTENT_LENGTH)
        {
            response
                .headers_mut()
                .insert(ASSET_REPRESENTATION_LENGTH_HEADER, length);
        }
        Ok(pin_response(response, pin))
    }
}

pub(crate) async fn serve_asset_plan(
    storage: &PlatformStorage,
    artifacts: &ArtifactStore,
    cache: Option<&Arc<ArtifactCache>>,
    deployment: &DeploymentRecord,
    plan: AssetResponsePlan,
) -> Result<Response, PlatformError> {
    let status = StatusCode::from_u16(plan.status).map_err(|_| internal())?;
    let mut response = Response::builder().status(status);
    for (name, value) in &plan.headers {
        let name = header::HeaderName::from_bytes(name.as_bytes()).map_err(|_| internal())?;
        let value = HeaderValue::from_str(value).map_err(|_| internal())?;
        response = response.header(name, value);
    }
    let body = match plan.entry {
        None => Body::empty(),
        Some(entry) => {
            let digest = parse_stored_digest(&entry.sha256)?;
            DeploymentAssetsRepository::new(storage.db()).authorize_blob(
                deployment.id,
                &deployment.worker_code_sha256,
                &digest,
                entry.size,
            )?;
            if plan.head {
                Body::empty()
            } else {
                let artifact = ArtifactRef::new(ARTIFACT_KEY_VERSION, &entry.sha256, entry.size)?;
                match cache {
                    Some(cache) => {
                        let pinned = cache
                            .acquire(artifacts, &artifact)
                            .await
                            .map_err(|error| map_asset_artifact_error(&error))?;
                        let file = pinned.file().try_clone().map_err(|_| {
                            PlatformError::new(
                                ErrorCode::AssetStorageUnavailable,
                                "verified asset cache file is unavailable",
                            )
                        })?;
                        let state = (tokio::fs::File::from_std(file), pinned, entry.size);
                        let stream = futures::stream::try_unfold(
                            state,
                            |(mut file, pinned, remaining)| async move {
                                if remaining == 0 {
                                    return Ok::<_, std::io::Error>(None);
                                }
                                let take =
                                    usize::try_from(remaining.min(64 * 1024)).unwrap_or(64 * 1024);
                                let mut buffer = vec![0_u8; take];
                                let read = file.read(&mut buffer).await?;
                                if read == 0 {
                                    return Err(std::io::Error::new(
                                        std::io::ErrorKind::UnexpectedEof,
                                        "verified asset cache entry is truncated",
                                    ));
                                }
                                buffer.truncate(read);
                                let remaining = remaining.saturating_sub(read as u64);
                                Ok(Some((Bytes::from(buffer), (file, pinned, remaining))))
                            },
                        );
                        Body::from_stream(stream)
                    }
                    None => Body::from(
                        artifacts
                            .open(&artifact)
                            .await
                            .map_err(|error| map_asset_artifact_error(&error))?,
                    ),
                }
            }
        }
    };
    response.body(body).map_err(|_| internal())
}

struct PinnedBody {
    inner: Body,
    pin: Option<DeploymentPin>,
}

impl HttpBody for PinnedBody {
    type Data = Bytes;
    type Error = axum::Error;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        let frame = Pin::new(&mut self.inner).poll_frame(cx);
        let finished = match &frame {
            Poll::Ready(None | Some(Err(_))) => true,
            Poll::Ready(Some(Ok(_))) => self.inner.is_end_stream(),
            Poll::Pending => false,
        };
        if finished {
            self.pin.take();
        }
        frame
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        self.inner.size_hint()
    }
}

pub(crate) fn pin_response(response: Response, pin: DeploymentPin) -> Response {
    let (parts, body) = response.into_parts();
    Response::from_parts(
        parts,
        Body::new(PinnedBody {
            inner: body,
            pin: Some(pin),
        }),
    )
}

fn parse_digest(value: &str) -> Result<[u8; 32], PlatformError> {
    if value.len() != 64
        || value
            .bytes()
            .any(|byte| !byte.is_ascii_hexdigit() || byte.is_ascii_uppercase())
    {
        return Err(protocol_error());
    }
    hex::decode(value)
        .map_err(|_| protocol_error())?
        .try_into()
        .map_err(|_| protocol_error())
}

fn parse_stored_digest(value: &str) -> Result<[u8; 32], PlatformError> {
    let decoded = hex::decode(value).map_err(|_| internal())?;
    decoded.try_into().map_err(|_| internal())
}

fn map_asset_artifact_error(error: &PlatformError) -> PlatformError {
    match error.code() {
        ErrorCode::ArtifactIntegrityError | ErrorCode::CacheEntryCorrupt => PlatformError::new(
            ErrorCode::AssetIntegrityError,
            "static asset failed integrity verification",
        ),
        _ => PlatformError::new(
            ErrorCode::AssetStorageUnavailable,
            "static asset provider is unavailable",
        ),
    }
}

fn asset_error(error: &PlatformError) -> Response {
    let (code, status) = match error.code() {
        ErrorCode::BindingProtocolError => {
            (ErrorCode::BindingProtocolError, StatusCode::BAD_REQUEST)
        }
        ErrorCode::AssetIntegrityError | ErrorCode::DeploymentInvariantViolation => (
            ErrorCode::AssetIntegrityError,
            StatusCode::INTERNAL_SERVER_ERROR,
        ),
        ErrorCode::AssetStorageUnavailable => (
            ErrorCode::AssetStorageUnavailable,
            StatusCode::SERVICE_UNAVAILABLE,
        ),
        _ => (
            ErrorCode::AssetStorageUnavailable,
            StatusCode::SERVICE_UNAVAILABLE,
        ),
    };
    let mut response = Response::new(Body::empty());
    *response.status_mut() = status;
    if let Ok(value) = HeaderValue::from_str(code.as_str()) {
        response.headers_mut().insert(ERROR_HEADER, value);
    }
    response
}

fn protocol_error() -> PlatformError {
    PlatformError::new(
        ErrorCode::BindingProtocolError,
        "asset binding request is invalid",
    )
}

fn invariant() -> PlatformError {
    PlatformError::new(
        ErrorCode::DeploymentInvariantViolation,
        "asset binding deployment authority is inconsistent",
    )
}

fn internal() -> PlatformError {
    PlatformError::new(ErrorCode::Internal, "static asset response failed")
}

#[cfg(test)]
#[path = "asset_backend_tests.rs"]
mod tests;
