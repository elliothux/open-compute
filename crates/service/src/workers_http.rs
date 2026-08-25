//! P0.2 Worker control API and public route ingress.

use crate::http::{HttpState, authorize};
use crate::metrics::DoFacetReloadReason;
use crate::runtime_bridge::{DispatchTarget, WorkerdTransport};
use axum::Router;
use axum::body::{Body, to_bytes};
use axum::extract::{Path, Request, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use bytes::Bytes;
use http_body_util::BodyExt as _;
use hyper::body::{Body as HttpBody, Frame, SizeHint};
use open_compute_artifacts::ArtifactStore;
use open_compute_core::{
    AccountId, BindingKind, DeploymentId, ErrorCode, PlatformError, RequestId, SecretString,
    WorkerId,
};
use open_compute_storage::{
    BindingRepository, DeploymentRecord, PlatformStorage, WorkerRepository,
};
use open_compute_workers::{
    BundleLimits, CreateDeploymentOutcome, CreateDeploymentRequest, DeploymentBindingInput,
    DeploymentBundle, DeploymentController, DeploymentPin, DeploymentPins, RuntimeValidator,
    StagedBundle,
};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::OpenOptions;
use std::future::Future;
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;
use std::pin::Pin;
use std::str::FromStr;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::{Duration, SystemTime};
use tokio::io::AsyncWriteExt;
use uuid::Uuid;

const IDEMPOTENCY_HEADER: &str = "idempotency-key";
pub(crate) const DEPLOYMENT_METADATA_HEADER: &str = "x-open-compute-deployment-metadata";
pub(crate) const MAX_DEPLOYMENT_METADATA_HEADER_BYTES: usize = 1024 * 1024;
const MAX_JSON_BODY: usize = 4096;
pub(crate) const HARD_MAX_BUNDLE_BODY: usize = 64 * 1024 * 1024;
const IDEMPOTENCY_TTL_MS: i64 = 24 * 60 * 60 * 1000;

/// Shared P0.2 HTTP authority.
#[derive(Clone)]
pub struct WorkerApiState {
    storage: Arc<PlatformStorage>,
    artifacts: ArtifactStore,
    transport: WorkerdTransport,
    pins: DeploymentPins,
    bundle_limits: BundleLimits,
    delete_drain_timeout: Duration,
}

impl std::fmt::Debug for WorkerApiState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkerApiState")
            .field("artifacts", &self.artifacts)
            .field("pins", &self.pins)
            .finish_non_exhaustive()
    }
}

impl WorkerApiState {
    /// Bind HTTP handlers to typed storage, artifact, and runtime capabilities.
    #[must_use]
    pub fn new(
        storage: Arc<PlatformStorage>,
        artifacts: ArtifactStore,
        transport: WorkerdTransport,
        pins: DeploymentPins,
        bundle_limits: BundleLimits,
        delete_drain_timeout: Duration,
    ) -> Self {
        Self {
            storage,
            artifacts,
            transport,
            pins,
            bundle_limits,
            delete_drain_timeout,
        }
    }

    /// Process-local dispatch/deletion pin registry.
    #[must_use]
    pub fn pins(&self) -> &DeploymentPins {
        &self.pins
    }
}

/// Router containing the stable P0.2 management surface.
pub fn control_router() -> Router<HttpState> {
    Router::new()
        .route(
            "/v1/accounts/{account_id}/workers",
            post(create_worker).get(list_workers),
        )
        .route(
            "/v1/accounts/{account_id}/workers/{worker_id}",
            get(get_worker).delete(delete_worker),
        )
        .route(
            "/v1/accounts/{account_id}/workers/{worker_id}/deployments",
            post(create_deployment).get(list_deployments),
        )
        .route(
            "/v1/accounts/{account_id}/workers/{worker_id}/deployments/{deployment_id}",
            get(get_deployment).delete(delete_deployment),
        )
        .route(
            "/v1/accounts/{account_id}/workers/{worker_id}/promotions",
            post(promote),
        )
        .route(
            "/v1/accounts/{account_id}/workers/{worker_id}/rollbacks",
            post(rollback),
        )
        .route(
            "/v1/accounts/{account_id}/workers/{worker_id}/routes",
            post(create_route).get(list_routes),
        )
        .route(
            "/v1/accounts/{account_id}/workers/{worker_id}/routes/{route_id}",
            delete(delete_route),
        )
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateWorkerBody {
    name: String,
}

async fn create_worker(
    State(state): State<HttpState>,
    Path(account): Path<String>,
    request: Request,
) -> Response {
    let request_id = request_id(&request);
    let Some(api) = authorized_api(&state, &request, request_id) else {
        return unauthorized_or_unavailable(&state, &request, request_id);
    };
    let account_id = match parse_account(&account) {
        Ok(id) => id,
        Err(error) => return error_response(error, request_id),
    };
    let key = match idempotency_key(&request) {
        Ok(key) => key,
        Err(error) => return error_response(error, request_id),
    };
    let body = match read_json::<CreateWorkerBody>(request, MAX_JSON_BODY).await {
        Ok(body) => body,
        Err(error) => return error_response(error, request_id),
    };
    let Ok(canonical) = serde_json::to_vec(&serde_json::json!({ "name": body.name })) else {
        return error_response(internal(), request_id);
    };
    let response = run_idempotent(
        api,
        account_id,
        "worker.create",
        &key,
        &canonical,
        request_id,
        None,
        || {
            let (worker, route) = WorkerRepository::new(api.storage.db()).create_worker(
                account_id,
                &body.name,
                request_id,
                now_ms(),
            )?;
            Ok(serde_json::json!({ "worker": worker, "defaultRoute": route }))
        },
    );
    idempotent_response(response, StatusCode::CREATED, request_id)
}

async fn list_workers(
    State(state): State<HttpState>,
    Path(account): Path<String>,
    request: Request,
) -> Response {
    let request_id = request_id(&request);
    let Some(api) = authorized_api(&state, &request, request_id) else {
        return unauthorized_or_unavailable(&state, &request, request_id);
    };
    let result = parse_account(&account)
        .and_then(|account_id| WorkerRepository::new(api.storage.db()).list_workers(account_id));
    result_response(
        result.map(|workers| serde_json::json!({ "workers": workers })),
        request_id,
    )
}

async fn get_worker(
    State(state): State<HttpState>,
    Path((account, worker)): Path<(String, String)>,
    request: Request,
) -> Response {
    let request_id = request_id(&request);
    let Some(api) = authorized_api(&state, &request, request_id) else {
        return unauthorized_or_unavailable(&state, &request, request_id);
    };
    let result = parse_ids(&account, &worker).and_then(|(account_id, worker_id)| {
        WorkerRepository::new(api.storage.db()).get_worker(account_id, worker_id)
    });
    result_response(
        result.map(|worker| serde_json::json!({ "worker": worker })),
        request_id,
    )
}

async fn delete_worker(
    State(state): State<HttpState>,
    Path((account, worker)): Path<(String, String)>,
    request: Request,
) -> Response {
    let request_id = request_id(&request);
    let Some(api) = authorized_api(&state, &request, request_id) else {
        return unauthorized_or_unavailable(&state, &request, request_id);
    };
    let (account_id, worker_id) = match parse_ids(&account, &worker) {
        Ok(ids) => ids,
        Err(error) => return error_response(error, request_id),
    };
    let key = match idempotency_key(&request) {
        Ok(key) => key,
        Err(error) => return error_response(error, request_id),
    };
    let scope = format!("worker.delete/{worker_id}");
    let response = run_idempotent(
        api,
        account_id,
        &scope,
        &key,
        worker_id.to_string().as_bytes(),
        request_id,
        None,
        || {
            WorkerRepository::new(api.storage.db()).delete_worker(
                account_id,
                worker_id,
                request_id,
                now_ms(),
            )?;
            Ok(serde_json::json!({ "workerId": worker_id, "state": "tombstoned" }))
        },
    );
    idempotent_response(response, StatusCode::ACCEPTED, request_id)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DeploymentMetadata {
    main_module: String,
    compatibility_date: String,
    #[serde(default)]
    compatibility_flags: Vec<String>,
    #[serde(default)]
    vars: BTreeMap<String, serde_json::Value>,
    #[serde(default)]
    secrets: BTreeMap<String, SecretString>,
    #[serde(default)]
    bindings: BTreeMap<String, DeploymentBindingInput>,
    #[serde(default = "default_limits")]
    limits: serde_json::Value,
    #[serde(default)]
    promote: bool,
}

async fn create_deployment(
    State(state): State<HttpState>,
    Path((account, worker)): Path<(String, String)>,
    request: Request,
) -> Response {
    let request_id = request_id(&request);
    let Some(api) = authorized_api(&state, &request, request_id) else {
        return unauthorized_or_unavailable(&state, &request, request_id);
    };
    let (account_id, worker_id) = match parse_ids(&account, &worker) {
        Ok(ids) => ids,
        Err(error) => return error_response(error, request_id),
    };
    let key = match idempotency_key(&request) {
        Ok(key) => key,
        Err(error) => return error_response(error, request_id),
    };
    let metadata = match deployment_metadata(&request) {
        Ok(metadata) => metadata,
        Err(error) => return error_response(error, request_id),
    };
    let body_limit = api
        .bundle_limits
        .max_artifact_bytes
        .min(HARD_MAX_BUNDLE_BODY);
    if request
        .headers()
        .get(header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<usize>().ok())
        .is_some_and(|size| size > body_limit)
    {
        return error_response(
            PlatformError::new(ErrorCode::BundleTooLarge, "deployment bundle exceeds limit"),
            request_id,
        );
    }
    let staged = match stage_bundle(
        request.into_body(),
        api.storage.data_dir().deployment_staging_dir(),
        api.bundle_limits,
        body_limit,
    )
    .await
    {
        Ok(staged) => staged,
        Err(error) => return error_response(error, request_id),
    };
    if staged.bundle.manifest().main_module != metadata.main_module {
        return error_response(
            PlatformError::new(
                ErrorCode::BundleInvalid,
                "metadata mainModule does not match the canonical bundle",
            ),
            request_id,
        );
    }
    let validator: Arc<dyn RuntimeValidator> = Arc::new(api.transport.clone());
    let controller = DeploymentController::new(
        &api.storage,
        api.artifacts.clone(),
        validator,
        api.bundle_limits,
    );
    let result = controller
        .create_deployment(CreateDeploymentRequest {
            account_id,
            worker_id,
            idempotency_key: key,
            bundle: DeploymentBundle::Staged(staged.bundle.clone()),
            compatibility_date: metadata.compatibility_date,
            compatibility_flags: metadata.compatibility_flags,
            vars: metadata.vars,
            secrets: metadata.secrets,
            bindings: metadata.bindings,
            limits: metadata.limits,
            promote: metadata.promote,
            request_id,
            now_ms: now_ms(),
        })
        .await;
    match result {
        Ok(CreateDeploymentOutcome::Applied(result)) => json_bytes(
            serde_json::to_vec(&result).unwrap_or_else(|_| b"{}".to_vec()),
            StatusCode::CREATED,
        ),
        Ok(CreateDeploymentOutcome::Replay(bytes)) => json_bytes(bytes, StatusCode::CREATED),
        Err(error) => error_response(error, request_id),
    }
}

async fn list_deployments(
    State(state): State<HttpState>,
    Path((account, worker)): Path<(String, String)>,
    request: Request,
) -> Response {
    let request_id = request_id(&request);
    let Some(api) = authorized_api(&state, &request, request_id) else {
        return unauthorized_or_unavailable(&state, &request, request_id);
    };
    let result = parse_ids(&account, &worker).and_then(|(account_id, worker_id)| {
        WorkerRepository::new(api.storage.db()).list_deployments(account_id, worker_id)
    });
    result_response(
        result.map(|deployments| {
            serde_json::json!({
                "deployments": deployments.iter().map(deployment_json).collect::<Vec<_>>()
            })
        }),
        request_id,
    )
}

async fn get_deployment(
    State(state): State<HttpState>,
    Path((account, worker, deployment)): Path<(String, String, String)>,
    request: Request,
) -> Response {
    let request_id = request_id(&request);
    let Some(api) = authorized_api(&state, &request, request_id) else {
        return unauthorized_or_unavailable(&state, &request, request_id);
    };
    let result = parse_deployment_ids(&account, &worker, &deployment).and_then(
        |(account_id, worker_id, deployment_id)| {
            WorkerRepository::new(api.storage.db()).get_deployment(
                account_id,
                worker_id,
                deployment_id,
            )
        },
    );
    result_response(
        result.map(|deployment| serde_json::json!({ "deployment": deployment_json(&deployment) })),
        request_id,
    )
}

async fn delete_deployment(
    State(state): State<HttpState>,
    Path((account, worker, deployment)): Path<(String, String, String)>,
    request: Request,
) -> Response {
    let request_id = request_id(&request);
    let Some(api) = authorized_api(&state, &request, request_id) else {
        return unauthorized_or_unavailable(&state, &request, request_id);
    };
    let (account_id, worker_id, deployment_id) =
        match parse_deployment_ids(&account, &worker, &deployment) {
            Ok(ids) => ids,
            Err(error) => return error_response(error, request_id),
        };
    let key = match idempotency_key(&request) {
        Ok(key) => key,
        Err(error) => return error_response(error, request_id),
    };
    let result =
        run_deployment_delete(api, account_id, worker_id, deployment_id, &key, request_id).await;
    idempotent_response(result, StatusCode::ACCEPTED, request_id)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PromotionBody {
    target_deployment_id: DeploymentId,
    #[serde(default)]
    expected_active_deployment_id: Option<DeploymentId>,
}

async fn promote(
    State(state): State<HttpState>,
    Path((account, worker)): Path<(String, String)>,
    request: Request,
) -> Response {
    promotion_impl(state, account, worker, request, false).await
}

async fn rollback(
    State(state): State<HttpState>,
    Path((account, worker)): Path<(String, String)>,
    request: Request,
) -> Response {
    promotion_impl(state, account, worker, request, true).await
}

async fn promotion_impl(
    state: HttpState,
    account: String,
    worker: String,
    request: Request,
    rollback: bool,
) -> Response {
    let request_id = request_id(&request);
    let Some(api) = authorized_api(&state, &request, request_id) else {
        return unauthorized_or_unavailable(&state, &request, request_id);
    };
    let (account_id, worker_id) = match parse_ids(&account, &worker) {
        Ok(ids) => ids,
        Err(error) => return error_response(error, request_id),
    };
    let key = match idempotency_key(&request) {
        Ok(key) => key,
        Err(error) => return error_response(error, request_id),
    };
    let body = match read_json::<PromotionBody>(request, MAX_JSON_BODY).await {
        Ok(body) => body,
        Err(error) => return error_response(error, request_id),
    };
    let canonical = serde_json::to_vec(&serde_json::json!({
        "targetDeploymentId": body.target_deployment_id,
        "expectedActiveDeploymentId": body.expected_active_deployment_id,
    }))
    .unwrap_or_default();
    let scope = format!(
        "deployment.{}/{}",
        if rollback { "rollback" } else { "promote" },
        worker_id
    );
    let metrics = state.metrics().clone();
    let response = run_idempotent_async(
        api,
        account_id,
        &scope,
        &key,
        &canonical,
        request_id,
        Some(body.target_deployment_id),
        || async {
            let repo = WorkerRepository::new(api.storage.db());
            let worker_before = repo.get_worker(account_id, worker_id)?;
            let deployment =
                repo.get_deployment(account_id, worker_id, body.target_deployment_id)?;
            let reloads_durable_objects = BindingRepository::new(api.storage.db())
                .deployment_bindings(deployment.id)?
                .iter()
                .any(|binding| binding.kind == BindingKind::DoNamespace);
            for route in repo.list_routes(account_id, worker_id)? {
                if let Some(entrypoint) = route.entrypoint {
                    api.transport
                        .probe_entrypoint(
                            open_compute_workers::ValidationCandidate {
                                account_id,
                                worker_id,
                                deployment_id: deployment.id,
                                worker_code_sha256: deployment.worker_code_sha256,
                            },
                            entrypoint,
                        )
                        .await?;
                }
            }
            let worker = repo.promote_checked(
                account_id,
                worker_id,
                body.target_deployment_id,
                body.expected_active_deployment_id,
                Some(worker_before.route_generation),
                request_id,
                now_ms(),
            )?;
            if reloads_durable_objects {
                metrics.inc_do_facet_reload(DoFacetReloadReason::Promotion);
            }
            Ok(serde_json::json!({ "worker": worker }))
        },
    )
    .await;
    idempotent_response(response, StatusCode::OK, request_id)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CreateRouteBody {
    hostname: String,
    path_prefix: String,
    #[serde(default)]
    entrypoint: Option<String>,
}

async fn create_route(
    State(state): State<HttpState>,
    Path((account, worker)): Path<(String, String)>,
    request: Request,
) -> Response {
    let request_id = request_id(&request);
    let Some(api) = authorized_api(&state, &request, request_id) else {
        return unauthorized_or_unavailable(&state, &request, request_id);
    };
    let (account_id, worker_id) = match parse_ids(&account, &worker) {
        Ok(ids) => ids,
        Err(error) => return error_response(error, request_id),
    };
    let key = match idempotency_key(&request) {
        Ok(key) => key,
        Err(error) => return error_response(error, request_id),
    };
    let body = match read_json::<CreateRouteBody>(request, MAX_JSON_BODY).await {
        Ok(body) => body,
        Err(error) => return error_response(error, request_id),
    };
    let hostname = match canonical_hostname(&body.hostname) {
        Ok(host) => host,
        Err(error) => return error_response(error, request_id),
    };
    if let Err(error) = validate_route_parts(&body.path_prefix, body.entrypoint.as_deref()) {
        return error_response(error, request_id);
    }
    let canonical = serde_json::to_vec(&serde_json::json!({
        "hostname": hostname,
        "pathPrefix": body.path_prefix,
        "entrypoint": body.entrypoint,
    }))
    .unwrap_or_default();
    let scope = format!("route.create/{worker_id}");
    let response = run_idempotent_async(
        api,
        account_id,
        &scope,
        &key,
        &canonical,
        request_id,
        None,
        || async {
            let repo = WorkerRepository::new(api.storage.db());
            let expected_active = if let Some(entrypoint) = body.entrypoint.as_ref() {
                let worker = repo.get_worker(account_id, worker_id)?;
                let deployment_id = worker.active_deployment_id.ok_or_else(|| {
                    PlatformError::new(
                        ErrorCode::DeploymentNotReady,
                        "a named route requires an active deployment",
                    )
                })?;
                let deployment = repo.get_deployment(account_id, worker_id, deployment_id)?;
                api.transport
                    .probe_entrypoint(
                        open_compute_workers::ValidationCandidate {
                            account_id,
                            worker_id,
                            deployment_id,
                            worker_code_sha256: deployment.worker_code_sha256,
                        },
                        entrypoint.clone(),
                    )
                    .await?;
                Some(deployment_id)
            } else {
                None
            };
            let route = repo.create_exact_route(
                account_id,
                worker_id,
                &hostname,
                &body.path_prefix,
                body.entrypoint.as_deref(),
                expected_active,
                request_id,
                now_ms(),
            )?;
            Ok(serde_json::json!({ "route": route }))
        },
    )
    .await;
    idempotent_response(response, StatusCode::CREATED, request_id)
}

async fn list_routes(
    State(state): State<HttpState>,
    Path((account, worker)): Path<(String, String)>,
    request: Request,
) -> Response {
    let request_id = request_id(&request);
    let Some(api) = authorized_api(&state, &request, request_id) else {
        return unauthorized_or_unavailable(&state, &request, request_id);
    };
    let result = parse_ids(&account, &worker).and_then(|(account_id, worker_id)| {
        WorkerRepository::new(api.storage.db()).list_routes(account_id, worker_id)
    });
    result_response(
        result.map(|routes| serde_json::json!({ "routes": routes })),
        request_id,
    )
}

async fn delete_route(
    State(state): State<HttpState>,
    Path((account, worker, route)): Path<(String, String, String)>,
    request: Request,
) -> Response {
    let request_id = request_id(&request);
    let Some(api) = authorized_api(&state, &request, request_id) else {
        return unauthorized_or_unavailable(&state, &request, request_id);
    };
    let (account_id, worker_id) = match parse_ids(&account, &worker) {
        Ok(ids) => ids,
        Err(error) => return error_response(error, request_id),
    };
    let key = match idempotency_key(&request) {
        Ok(key) => key,
        Err(error) => return error_response(error, request_id),
    };
    let scope = format!("route.delete/{worker_id}/{route}");
    let response = run_idempotent(
        api,
        account_id,
        &scope,
        &key,
        route.as_bytes(),
        request_id,
        None,
        || {
            WorkerRepository::new(api.storage.db()).delete_route(
                account_id,
                worker_id,
                &route,
                request_id,
                now_ms(),
            )?;
            Ok(serde_json::json!({ "routeId": route, "state": "tombstoned" }))
        },
    );
    idempotent_response(response, StatusCode::ACCEPTED, request_id)
}

/// Fallback for the public listener: resolve DB route, freeze deployment, stream through workerd.
pub async fn public_ingress(State(state): State<HttpState>, request: Request) -> Response {
    let request_id = request_id(&request);
    let Some(api) = state.worker_api() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let hostname = match request
        .headers()
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| PlatformError::new(ErrorCode::RouteNotFound, "Host header is required"))
        .and_then(canonical_request_host)
    {
        Ok(hostname) => hostname,
        Err(error) => return error_response(error, request_id),
    };
    let repo = WorkerRepository::new(api.storage.db());
    let snapshot = match repo.resolve_route(Some(&hostname), request.uri().path()) {
        Ok(snapshot) => snapshot,
        Err(error) if error.code() == ErrorCode::RouteNotFound => {
            match repo.resolve_route(None, request.uri().path()) {
                Ok(snapshot) => snapshot,
                Err(error) => return error_response(error, request_id),
            }
        }
        Err(error) => return error_response(error, request_id),
    };
    let pin = match api.pins.pin(snapshot.deployment.id) {
        Ok(pin) => pin,
        Err(error) => return error_response(error, request_id),
    };
    let target = DispatchTarget {
        account_id: snapshot.route.account_id,
        worker_id: snapshot.route.worker_id,
        deployment_id: snapshot.deployment.id,
        worker_code_sha256: hex::encode(snapshot.deployment.worker_code_sha256),
        entrypoint: snapshot.route.entrypoint,
        route_generation: i64::try_from(snapshot.worker.route_generation).unwrap_or(i64::MAX),
        request_id,
    };
    match api.transport.dispatch(target, request).await {
        Ok(response) => pin_response(response, pin),
        Err(error) => error_response(error, request_id),
    }
}

struct PinnedBody {
    inner: Body,
    _pin: DeploymentPin,
}

impl HttpBody for PinnedBody {
    type Data = Bytes;
    type Error = axum::Error;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        Pin::new(&mut self.inner).poll_frame(cx)
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        self.inner.size_hint()
    }
}

fn pin_response(response: Response, pin: DeploymentPin) -> Response {
    let (parts, body) = response.into_parts();
    Response::from_parts(
        parts,
        Body::new(PinnedBody {
            inner: body,
            _pin: pin,
        }),
    )
}

fn authorized_api<'a>(
    state: &'a HttpState,
    request: &Request,
    _request_id: RequestId,
) -> Option<&'a Arc<WorkerApiState>> {
    if authorize(state, request) {
        state.worker_api()
    } else {
        None
    }
}

fn unauthorized_or_unavailable(
    state: &HttpState,
    request: &Request,
    request_id: RequestId,
) -> Response {
    if !authorize(state, request) {
        error_response(
            PlatformError::new(
                ErrorCode::AdminAuthRequired,
                "admin authentication is required",
            ),
            request_id,
        )
    } else {
        StatusCode::NOT_FOUND.into_response()
    }
}

struct StagingCleanup {
    path: PathBuf,
}

impl Drop for StagingCleanup {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

struct StagedUpload {
    bundle: StagedBundle,
    _cleanup: StagingCleanup,
}

async fn stage_bundle(
    mut body: Body,
    directory: PathBuf,
    limits: BundleLimits,
    body_limit: usize,
) -> Result<StagedUpload, PlatformError> {
    let path = directory.join(format!("{}.upload", Uuid::now_v7()));
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)
        .map_err(|_| {
            PlatformError::new(
                ErrorCode::DiskHardLimit,
                "failed to create deployment staging file",
            )
        })?;
    let cleanup = StagingCleanup { path: path.clone() };
    let mut file = tokio::fs::File::from_std(file);
    let mut written = 0_usize;
    while let Some(frame) = body.frame().await {
        let frame = frame.map_err(|_| {
            PlatformError::new(ErrorCode::BundleInvalid, "deployment upload stream failed")
        })?;
        let Ok(data) = frame.into_data() else {
            continue;
        };
        written = written.checked_add(data.len()).ok_or_else(|| {
            PlatformError::new(ErrorCode::BundleTooLarge, "deployment bundle exceeds limit")
        })?;
        if written > body_limit {
            return Err(PlatformError::new(
                ErrorCode::BundleTooLarge,
                "deployment bundle exceeds limit",
            ));
        }
        file.write_all(&data).await.map_err(|_| {
            PlatformError::new(
                ErrorCode::DiskHardLimit,
                "failed to write deployment staging file",
            )
        })?;
    }
    file.sync_all().await.map_err(|_| {
        PlatformError::new(
            ErrorCode::DiskHardLimit,
            "failed to persist deployment staging file",
        )
    })?;
    drop(file);
    let bundle = StagedBundle::open(path, limits)?;
    Ok(StagedUpload {
        bundle,
        _cleanup: cleanup,
    })
}

async fn read_json<T: for<'de> Deserialize<'de>>(
    request: Request,
    limit: usize,
) -> Result<T, PlatformError> {
    let bytes = to_bytes(request.into_body(), limit).await.map_err(|_| {
        PlatformError::new(
            ErrorCode::LimitInvalid,
            "control request body exceeds limit",
        )
    })?;
    serde_json::from_slice(&bytes).map_err(|_| {
        PlatformError::new(ErrorCode::ConfigInvalid, "control request JSON is invalid")
    })
}

fn deployment_metadata(request: &Request) -> Result<DeploymentMetadata, PlatformError> {
    let raw = request
        .headers()
        .get(DEPLOYMENT_METADATA_HEADER)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| {
            PlatformError::new(
                ErrorCode::ConfigInvalid,
                "deployment metadata header is required",
            )
        })?;
    if raw.len() > MAX_DEPLOYMENT_METADATA_HEADER_BYTES {
        return Err(PlatformError::new(
            ErrorCode::LimitInvalid,
            "deployment metadata header exceeds limit",
        ));
    }
    serde_json::from_str(raw)
        .map_err(|_| PlatformError::new(ErrorCode::ConfigInvalid, "deployment metadata is invalid"))
}

fn default_limits() -> serde_json::Value {
    serde_json::json!({ "profile": "default" })
}

fn parse_account(value: &str) -> Result<AccountId, PlatformError> {
    AccountId::from_str(value)
        .map_err(|_| PlatformError::new(ErrorCode::ConfigInvalid, "account ID is invalid"))
}

fn parse_ids(value: &str, worker: &str) -> Result<(AccountId, WorkerId), PlatformError> {
    let account = parse_account(value)?;
    let worker = WorkerId::from_str(worker)
        .map_err(|_| PlatformError::new(ErrorCode::ConfigInvalid, "Worker ID is invalid"))?;
    Ok((account, worker))
}

fn parse_deployment_ids(
    account: &str,
    worker: &str,
    deployment: &str,
) -> Result<(AccountId, WorkerId, DeploymentId), PlatformError> {
    let (account, worker) = parse_ids(account, worker)?;
    let deployment = DeploymentId::from_str(deployment)
        .map_err(|_| PlatformError::new(ErrorCode::ConfigInvalid, "deployment ID is invalid"))?;
    Ok((account, worker, deployment))
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

#[allow(clippy::too_many_arguments)]
fn run_idempotent(
    api: &WorkerApiState,
    account_id: AccountId,
    scope: &str,
    key: &str,
    canonical_request: &[u8],
    _request_id: RequestId,
    deployment_ref: Option<DeploymentId>,
    operation: impl FnOnce() -> Result<serde_json::Value, PlatformError>,
) -> Result<Vec<u8>, PlatformError> {
    let mut input = Vec::with_capacity(scope.len() + canonical_request.len() + 1);
    input.extend_from_slice(scope.as_bytes());
    input.push(0);
    input.extend_from_slice(canonical_request);
    let fingerprint = api.storage.crypto().fingerprint_request(&input);
    let repo = WorkerRepository::new(api.storage.db());
    match repo.reserve_idempotency(
        account_id,
        scope,
        key,
        api.storage.crypto().fingerprint_key_id(),
        &fingerprint,
        now_ms(),
        now_ms().saturating_add(IDEMPOTENCY_TTL_MS),
    )? {
        open_compute_storage::IdempotencyReservation::Complete(bytes) => return Ok(bytes),
        open_compute_storage::IdempotencyReservation::Running => {
            return Err(PlatformError::new(
                ErrorCode::IdempotencyConflict,
                "idempotent operation is still running",
            ));
        }
        open_compute_storage::IdempotencyReservation::Failed(bytes) => {
            return Err(replayed_failure(&bytes));
        }
        open_compute_storage::IdempotencyReservation::Reserved => {}
    }
    match operation() {
        Ok(value) => {
            let response = serde_json::to_vec(&value).map_err(|_| internal())?;
            if let Some(deployment_id) = deployment_ref {
                repo.complete_idempotency_with_deployment_ref(
                    account_id,
                    scope,
                    key,
                    &fingerprint,
                    &response,
                    deployment_id,
                    &idempotency_ref_id(account_id, scope, key),
                    now_ms(),
                )?;
            } else {
                repo.complete_idempotency(account_id, scope, key, &fingerprint, &response)?;
            }
            Ok(response)
        }
        Err(error) => {
            let failure = serde_json::to_vec(&serde_json::json!({
                "code": error.code().as_str()
            }))
            .map_err(|_| internal())?;
            repo.fail_idempotency(account_id, scope, key, &fingerprint, &failure)?;
            Err(error)
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_idempotent_async<F, Fut>(
    api: &WorkerApiState,
    account_id: AccountId,
    scope: &str,
    key: &str,
    canonical_request: &[u8],
    _request_id: RequestId,
    deployment_ref: Option<DeploymentId>,
    operation: F,
) -> Result<Vec<u8>, PlatformError>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<serde_json::Value, PlatformError>>,
{
    let mut input = Vec::with_capacity(scope.len() + canonical_request.len() + 1);
    input.extend_from_slice(scope.as_bytes());
    input.push(0);
    input.extend_from_slice(canonical_request);
    let fingerprint = api.storage.crypto().fingerprint_request(&input);
    let repo = WorkerRepository::new(api.storage.db());
    match repo.reserve_idempotency(
        account_id,
        scope,
        key,
        api.storage.crypto().fingerprint_key_id(),
        &fingerprint,
        now_ms(),
        now_ms().saturating_add(IDEMPOTENCY_TTL_MS),
    )? {
        open_compute_storage::IdempotencyReservation::Complete(bytes) => return Ok(bytes),
        open_compute_storage::IdempotencyReservation::Running => {
            return Err(PlatformError::new(
                ErrorCode::IdempotencyConflict,
                "idempotent operation is still running",
            ));
        }
        open_compute_storage::IdempotencyReservation::Failed(bytes) => {
            return Err(replayed_failure(&bytes));
        }
        open_compute_storage::IdempotencyReservation::Reserved => {}
    }
    match operation().await {
        Ok(value) => {
            let response = serde_json::to_vec(&value).map_err(|_| internal())?;
            if let Some(deployment_id) = deployment_ref {
                repo.complete_idempotency_with_deployment_ref(
                    account_id,
                    scope,
                    key,
                    &fingerprint,
                    &response,
                    deployment_id,
                    &idempotency_ref_id(account_id, scope, key),
                    now_ms(),
                )?;
            } else {
                repo.complete_idempotency(account_id, scope, key, &fingerprint, &response)?;
            }
            Ok(response)
        }
        Err(error) => {
            let failure = serde_json::to_vec(&serde_json::json!({
                "code": error.code().as_str()
            }))
            .map_err(|_| internal())?;
            repo.fail_idempotency(account_id, scope, key, &fingerprint, &failure)?;
            Err(error)
        }
    }
}

async fn run_deployment_delete(
    api: &WorkerApiState,
    account_id: AccountId,
    worker_id: WorkerId,
    deployment_id: DeploymentId,
    key: &str,
    request_id: RequestId,
) -> Result<Vec<u8>, PlatformError> {
    let scope = format!("deployment.delete/{worker_id}/{deployment_id}");
    let canonical = serde_json::to_vec(&serde_json::json!({
        "workerId": worker_id,
        "deploymentId": deployment_id,
    }))
    .map_err(|_| internal())?;
    let mut input = Vec::with_capacity(scope.len() + canonical.len() + 1);
    input.extend_from_slice(scope.as_bytes());
    input.push(0);
    input.extend_from_slice(&canonical);
    let fingerprint = api.storage.crypto().fingerprint_request(&input);
    let repo = WorkerRepository::new(api.storage.db());
    let operation_now = now_ms();
    let _ = repo.prune_expired_idempotency(operation_now, 100)?;
    match repo.reserve_idempotency(
        account_id,
        &scope,
        key,
        api.storage.crypto().fingerprint_key_id(),
        &fingerprint,
        operation_now,
        operation_now.saturating_add(IDEMPOTENCY_TTL_MS),
    )? {
        open_compute_storage::IdempotencyReservation::Complete(response) => return Ok(response),
        open_compute_storage::IdempotencyReservation::Running => {
            return Err(PlatformError::new(
                ErrorCode::IdempotencyConflict,
                "idempotent operation is still running",
            ));
        }
        open_compute_storage::IdempotencyReservation::Failed(response) => {
            return Err(replayed_failure(&response));
        }
        open_compute_storage::IdempotencyReservation::Reserved => {}
    }

    let operation = async {
        repo.begin_deployment_delete(account_id, worker_id, deployment_id)?;
        api.pins
            .fence_and_wait(deployment_id, api.delete_drain_timeout)
            .await?;
        repo.finalize_deployment_delete(
            account_id,
            worker_id,
            deployment_id,
            request_id,
            now_ms(),
        )?;
        serde_json::to_vec(&serde_json::json!({
            "deploymentId": deployment_id,
            "state": "tombstoned"
        }))
        .map_err(|_| internal())
    }
    .await;
    match operation {
        Ok(response) => {
            api.pins.retire_fence(deployment_id);
            repo.complete_idempotency(account_id, &scope, key, &fingerprint, &response)?;
            Ok(response)
        }
        Err(error) => {
            let failure = serde_json::to_vec(&serde_json::json!({
                "code": error.code().as_str()
            }))
            .map_err(|_| internal())?;
            repo.fail_idempotency(account_id, &scope, key, &fingerprint, &failure)?;
            Err(error)
        }
    }
}

fn replayed_failure(response: &[u8]) -> PlatformError {
    let code = serde_json::from_slice::<serde_json::Value>(response)
        .ok()
        .and_then(|value| value.get("code")?.as_str().map(str::to_owned))
        .unwrap_or_else(|| ErrorCode::Internal.as_str().to_owned());
    PlatformError::new(error_code(&code), "idempotent operation previously failed")
}

fn idempotency_ref_id(account_id: AccountId, scope: &str, key: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"open-compute/deployment-referrer/v1\0");
    hasher.update(account_id.to_string().as_bytes());
    hasher.update([0]);
    hasher.update(scope.as_bytes());
    hasher.update([0]);
    hasher.update(key.as_bytes());
    hex::encode(hasher.finalize())
}

fn canonical_hostname(value: &str) -> Result<String, PlatformError> {
    if value.is_empty() || value.len() > 253 || value.contains(['/', '@', '#', '?']) {
        return Err(PlatformError::new(
            ErrorCode::ConfigInvalid,
            "route hostname is invalid",
        ));
    }
    url::Host::parse(value)
        .map(|host| host.to_string().trim_end_matches('.').to_ascii_lowercase())
        .map_err(|_| PlatformError::new(ErrorCode::ConfigInvalid, "route hostname is invalid"))
}

fn canonical_request_host(value: &str) -> Result<String, PlatformError> {
    let authority = value.parse::<axum::http::uri::Authority>().map_err(|_| {
        PlatformError::new(ErrorCode::RouteNotFound, "public request Host is invalid")
    })?;
    canonical_hostname(authority.host())
}

fn validate_route_parts(path: &str, entrypoint: Option<&str>) -> Result<(), PlatformError> {
    if path.is_empty()
        || path.len() > 2048
        || !path.starts_with('/')
        || path.contains(['?', '#', '\0'])
    {
        return Err(PlatformError::new(
            ErrorCode::ConfigInvalid,
            "route pathPrefix is invalid",
        ));
    }
    if entrypoint.is_some_and(|value| {
        value.is_empty()
            || value.len() > 128
            || !value
                .as_bytes()
                .first()
                .is_some_and(|byte| byte.is_ascii_alphabetic() || matches!(byte, b'_' | b'$'))
            || value
                .as_bytes()
                .get(1..)
                .unwrap_or_default()
                .iter()
                .any(|byte| !byte.is_ascii_alphanumeric() && !matches!(byte, b'_' | b'$'))
    }) {
        return Err(PlatformError::new(
            ErrorCode::ConfigInvalid,
            "route entrypoint is invalid",
        ));
    }
    Ok(())
}

fn deployment_json(deployment: &DeploymentRecord) -> serde_json::Value {
    serde_json::json!({
        "id": deployment.id,
        "workerId": deployment.worker_id,
        "versionNumber": deployment.version_number,
        "state": deployment.state,
        "artifactSha256": hex::encode(deployment.artifact_sha256),
        "artifactSize": deployment.artifact_size,
        "artifactSchemaVersion": deployment.artifact_schema_version,
        "mainModule": deployment.main_module,
        "compatibilityDate": deployment.compatibility_date,
        "compatibilityFlags": deployment.compatibility_flags,
        "limits": deployment.limits,
        "workerCodeSha256": hex::encode(deployment.worker_code_sha256),
        "loaderSchemaVersion": deployment.loader_schema_version,
        "createdAtMs": deployment.created_at_ms,
        "readyAtMs": deployment.ready_at_ms,
        "rejectedAtMs": deployment.rejected_at_ms,
        "rejectionCode": deployment.rejection_code,
        "deletedAtMs": deployment.deleted_at_ms,
    })
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
        .duration_since(SystemTime::UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
        .unwrap_or(i64::MAX)
}

fn result_response(
    result: Result<serde_json::Value, PlatformError>,
    request_id: RequestId,
) -> Response {
    match result {
        Ok(value) => axum::Json(value).into_response(),
        Err(error) => error_response(error, request_id),
    }
}

fn idempotent_response(
    result: Result<Vec<u8>, PlatformError>,
    status: StatusCode,
    request_id: RequestId,
) -> Response {
    match result {
        Ok(bytes) => json_bytes(bytes, status),
        Err(error) => error_response(error, request_id),
    }
}

fn json_bytes(bytes: Vec<u8>, status: StatusCode) -> Response {
    let mut response = (status, bytes).into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    response
}

#[allow(clippy::needless_pass_by_value)]
fn error_response(error: PlatformError, request_id: RequestId) -> Response {
    let status = match error.code() {
        ErrorCode::AdminAuthRequired => StatusCode::UNAUTHORIZED,
        ErrorCode::AccountNotFound
        | ErrorCode::WorkerNotFound
        | ErrorCode::DeploymentNotFound
        | ErrorCode::RouteNotFound
        | ErrorCode::EntrypointNotFound => StatusCode::NOT_FOUND,
        ErrorCode::WorkerNameConflict | ErrorCode::RouteConflict => StatusCode::CONFLICT,
        ErrorCode::DeploymentNotReady
        | ErrorCode::DeploymentActive
        | ErrorCode::DeploymentReferenced
        | ErrorCode::IdempotencyConflict => StatusCode::CONFLICT,
        ErrorCode::BundleTooLarge | ErrorCode::LimitInvalid => StatusCode::PAYLOAD_TOO_LARGE,
        ErrorCode::BundleRuntimeInvalid | ErrorCode::CompatibilityUnsupported => {
            StatusCode::UNPROCESSABLE_ENTITY
        }
        ErrorCode::RuntimeUnavailable | ErrorCode::ArtifactUnavailable => {
            StatusCode::SERVICE_UNAVAILABLE
        }
        ErrorCode::ResourceLimitExceeded => StatusCode::TOO_MANY_REQUESTS,
        ErrorCode::Internal
        | ErrorCode::RuntimeResultUnknown
        | ErrorCode::DeploymentInvariantViolation
        | ErrorCode::ArtifactIntegrityError => StatusCode::INTERNAL_SERVER_ERROR,
        _ => StatusCode::BAD_REQUEST,
    };
    (
        status,
        axum::Json(serde_json::json!({
            "ok": false,
            "error": {
                "code": error.code().as_str(),
                "message": error.message(),
                "requestId": request_id,
            }
        })),
    )
        .into_response()
}

fn error_code(value: &str) -> ErrorCode {
    [
        ErrorCode::AccountNotFound,
        ErrorCode::WorkerNameConflict,
        ErrorCode::WorkerNotFound,
        ErrorCode::WorkerDeleted,
        ErrorCode::DeploymentNotFound,
        ErrorCode::DeploymentNotReady,
        ErrorCode::DeploymentActive,
        ErrorCode::DeploymentReferenced,
        ErrorCode::DeploymentInvariantViolation,
        ErrorCode::BundleInvalid,
        ErrorCode::BundleTooLarge,
        ErrorCode::BundleRuntimeInvalid,
        ErrorCode::CompatibilityUnsupported,
        ErrorCode::ArtifactUnavailable,
        ErrorCode::ArtifactIntegrityError,
        ErrorCode::RouteNotFound,
        ErrorCode::RouteConflict,
        ErrorCode::EntrypointNotFound,
        ErrorCode::SecretInvalid,
        ErrorCode::IdempotencyConflict,
        ErrorCode::RuntimeUnavailable,
        ErrorCode::RuntimeResultUnknown,
        ErrorCode::ResourceLimitExceeded,
        ErrorCode::Internal,
    ]
    .into_iter()
    .find(|code| code.as_str() == value)
    .unwrap_or(ErrorCode::Internal)
}

fn internal() -> PlatformError {
    PlatformError::new(ErrorCode::Internal, "internal control operation failed")
}

#[cfg(test)]
#[path = "workers_http_tests.rs"]
mod tests;
