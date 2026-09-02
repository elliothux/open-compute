//! P0.2 Worker control API and public route ingress.

use crate::asset_backend::pin_response;
use crate::http::{HttpState, ProductErrorCode, authorize};
use crate::metrics::DoFacetReloadReason;
use crate::runtime_bridge::{DispatchTarget, WorkerdTransport};
use axum::body::{Body, to_bytes};
use axum::extract::{Path, Query, Request, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use http_body_util::BodyExt as _;
use open_compute_artifacts::{ArtifactCache, ArtifactStore};
use open_compute_core::{
    AccountId, BindingKind, ErrorCode, PlatformError, RequestId, SecretString, VersionId, WorkerId,
};
use open_compute_storage::{
    BindingRepository, CatalogDirection, CatalogSort, DEFAULT_CATALOG_LIST_LIMIT, PlatformStorage,
    VersionRecord, WorkerRepository, decode_catalog_cursor, normalize_catalog_limit,
};
use open_compute_workers::{
    BundleLimits, CreateVersionOutcome, CreateVersionRequest, ProductPromotionCoordinator,
    ProductPromotionRequest, QueueConsumerInput, RuntimeValidator, StagedBundle,
    VersionBindingInput, VersionBundle, VersionContent, VersionController, VersionPins,
    VersionRuntimeFeatures, VersionServiceInput,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::collections::HashMap;
use std::fs::OpenOptions;
use std::future::Future;
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::{Duration, SystemTime};
use tokio::io::AsyncWriteExt;
use uuid::Uuid;

mod control;
pub use control::control_router;
mod uploads;
use uploads::{
    abort_version_upload, create_version_upload, finalize_version_upload, get_version_upload,
    put_version_upload_object,
};

const IDEMPOTENCY_HEADER: &str = "idempotency-key";
pub(crate) const VERSION_METADATA_HEADER: &str = "x-open-compute-version-metadata";
pub(crate) const MAX_VERSION_METADATA_HEADER_BYTES: usize = 1024 * 1024;
const MAX_JSON_BODY: usize = 4096;
pub(crate) const HARD_MAX_BUNDLE_BODY: usize = 64 * 1024 * 1024;
const IDEMPOTENCY_TTL_MS: i64 = 24 * 60 * 60 * 1000;

/// Shared P0.2 HTTP authority.
#[derive(Clone)]
pub struct WorkerApiState {
    storage: Arc<PlatformStorage>,
    artifacts: ArtifactStore,
    cache: Option<Arc<ArtifactCache>>,
    response_cache: Option<Arc<open_compute_storage::CacheManager>>,
    transport: WorkerdTransport,
    pins: VersionPins,
    bundle_limits: BundleLimits,
    delete_drain_timeout: Duration,
    max_queue_consumer_concurrency: u32,
    product_promoter: Option<Arc<dyn ProductPromotionCoordinator>>,
    finalize_locks: Arc<[tokio::sync::Mutex<()>; 16]>,
    traffic: Arc<WorkerTrafficRegistry>,
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
        pins: VersionPins,
        bundle_limits: BundleLimits,
        delete_drain_timeout: Duration,
    ) -> Self {
        Self {
            storage,
            artifacts,
            cache: None,
            response_cache: None,
            transport,
            pins,
            bundle_limits,
            delete_drain_timeout,
            max_queue_consumer_concurrency: 32,
            product_promoter: None,
            finalize_locks: Arc::new(std::array::from_fn(|_| tokio::sync::Mutex::new(()))),
            traffic: Arc::new(WorkerTrafficRegistry::default()),
        }
    }

    /// Attach the verified local artifact cache used for backpressured asset bodies.
    #[must_use]
    pub fn with_cache(mut self, cache: Arc<ArtifactCache>) -> Self {
        self.cache = Some(cache);
        self
    }

    /// Attach the response-cache authority for Worker deletion fencing and cleanup.
    #[must_use]
    pub fn with_response_cache(
        mut self,
        response_cache: Arc<open_compute_storage::CacheManager>,
    ) -> Self {
        self.response_cache = Some(response_cache);
        self
    }

    /// Apply the operator-local Queue consumer concurrency ceiling.
    #[must_use]
    pub fn with_queue_consumer_limit(mut self, maximum: u32) -> Self {
        self.max_queue_consumer_concurrency = maximum.max(1);
        self
    }

    /// Attach the Queue/Cron cross-database promotion owner.
    #[must_use]
    pub fn with_product_promoter(mut self, promoter: Arc<dyn ProductPromotionCoordinator>) -> Self {
        self.product_promoter = Some(promoter);
        self
    }

    /// Process-local dispatch/deletion pin registry.
    #[must_use]
    pub fn pins(&self) -> &VersionPins {
        &self.pins
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct WorkerTrafficAccumulator {
    requests: u64,
    errors: u64,
    total_latency_micros: u64,
    last_status: Option<u16>,
}

#[derive(Clone, Copy, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkerTrafficSummary {
    requests: u64,
    errors: u64,
    average_latency_ms: f64,
    last_status: Option<u16>,
}

#[derive(Debug, Default)]
struct WorkerTrafficRegistry {
    entries: Mutex<HashMap<WorkerId, WorkerTrafficAccumulator>>,
}

impl WorkerTrafficRegistry {
    fn observe(&self, worker_id: WorkerId, status: u16, elapsed: Duration) {
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let entry = entries.entry(worker_id).or_default();
        entry.requests = entry.requests.saturating_add(1);
        if status >= 500 {
            entry.errors = entry.errors.saturating_add(1);
        }
        entry.total_latency_micros = entry
            .total_latency_micros
            .saturating_add(u64::try_from(elapsed.as_micros()).unwrap_or(u64::MAX));
        entry.last_status = Some(status);
    }

    fn summary(&self, worker_id: WorkerId) -> WorkerTrafficSummary {
        let entries = self
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let value = entries.get(&worker_id).copied().unwrap_or_default();
        WorkerTrafficSummary {
            requests: value.requests,
            errors: value.errors,
            average_latency_ms: if value.requests == 0 {
                0.0
            } else {
                value.total_latency_micros as f64 / value.requests as f64 / 1_000.0
            },
            last_status: value.last_status,
        }
    }

    fn remove(&self, worker_id: WorkerId) {
        self.entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&worker_id);
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkerCatalogEntry {
    #[serde(flatten)]
    worker: open_compute_storage::WorkerRecord,
    route_count: usize,
    primary_route: Option<open_compute_storage::RouteRecord>,
    version_source: Option<&'static str>,
    traffic: WorkerTrafficSummary,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateWorkerBody {
    name: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ListWorkersQuery {
    search: Option<String>,
    deployed: Option<bool>,
    sort: Option<CatalogSort>,
    direction: Option<CatalogDirection>,
    cursor: Option<String>,
    limit: Option<u16>,
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
            let _admission = api.storage.reserve_mutation(64 * 1024)?;
            let (worker, route) = WorkerRepository::new(api.storage.db()).create_worker(
                account_id,
                &body.name,
                request_id,
                now_ms(),
                api.storage.hardening().max_workers_per_account,
            )?;
            Ok(serde_json::json!({ "worker": worker, "defaultRoute": route }))
        },
    );
    idempotent_response(response, StatusCode::CREATED, request_id)
}

async fn list_workers(
    State(state): State<HttpState>,
    Path(account): Path<String>,
    query: Result<Query<ListWorkersQuery>, axum::extract::rejection::QueryRejection>,
    request: Request,
) -> Response {
    let request_id = request_id(&request);
    let Some(api) = authorized_api(&state, &request, request_id) else {
        return unauthorized_or_unavailable(&state, &request, request_id);
    };
    let Ok(Query(query)) = query else {
        return error_response(
            PlatformError::new(ErrorCode::ConfigInvalid, "Worker list query is invalid"),
            request_id,
        );
    };
    let account_id = match parse_account(&account) {
        Ok(value) => value,
        Err(error) => return error_response(error, request_id),
    };
    let after = match query.cursor.as_deref() {
        None => None,
        Some(cursor) => match decode_catalog_cursor(cursor) {
            Ok(value) => Some(value),
            Err(error) => return error_response(error, request_id),
        },
    };
    let limit = normalize_catalog_limit(query.limit.unwrap_or(DEFAULT_CATALOG_LIST_LIMIT));
    let sort = query.sort.unwrap_or(CatalogSort::UpdatedAt);
    let direction = query.direction.unwrap_or(CatalogDirection::Desc);
    let search = query
        .search
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let storage = api.storage.clone();
    let traffic = api.traffic.clone();
    match tokio::task::spawn_blocking(move || {
        let repo = WorkerRepository::new(storage.db());
        let page = repo.list_workers_page(
            account_id,
            search.as_deref(),
            query.deployed,
            sort,
            direction,
            after,
            limit,
        )?;
        let mut workers = Vec::with_capacity(page.items.len());
        for worker in page.items {
            let routes = repo.list_routes(account_id, worker.id)?;
            workers.push(WorkerCatalogEntry {
                route_count: routes.len(),
                primary_route: routes.into_iter().next(),
                version_source: worker.active_version_id.map(|_| "operator_api"),
                traffic: traffic.summary(worker.id),
                worker,
            });
        }
        Ok::<_, PlatformError>((workers, page.next_cursor))
    })
    .await
    {
        Ok(Ok((workers, next_cursor))) => {
            let list_complete = next_cursor.is_none();
            result_response(
                Ok(serde_json::json!({
                    "workers": workers,
                    "cursor": next_cursor,
                    "listComplete": list_complete,
                })),
                request_id,
            )
        }
        Ok(Err(error)) => error_response(error, request_id),
        Err(_) => error_response(internal(), request_id),
    }
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
        WorkerRepository::new(api.storage.db()).get_tenant_worker(account_id, worker_id)
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
    let canonical =
        serde_json::to_vec(&serde_json::json!({ "workerId": worker_id })).unwrap_or_default();
    let response = run_idempotent_async(
        api,
        account_id,
        &scope,
        &key,
        &canonical,
        request_id,
        None,
        || async {
            let _admission = api.storage.reserve_mutation(64 * 1024)?;
            let repo = WorkerRepository::new(api.storage.db());
            let versions = repo
                .list_versions(account_id, worker_id)?
                .into_iter()
                .filter(|version| version.deleted_at_ms.is_none())
                .map(|version| version.id)
                .collect::<Vec<_>>();
            if let Err(error) = api
                .pins
                .fence_many_and_wait(&versions, api.delete_drain_timeout)
                .await
            {
                for version in &versions {
                    api.pins.unfence(*version);
                }
                return Err(error);
            }
            if let Some(cache) = &api.response_cache
                && let Err(error) = cache.purge_worker(account_id, worker_id, now_ms())
            {
                for version in &versions {
                    api.pins.unfence(*version);
                }
                return Err(error);
            }
            if let Err(error) =
                repo.delete_worker(account_id, worker_id, &versions, request_id, now_ms())
            {
                for version in &versions {
                    api.pins.unfence(*version);
                }
                return Err(error);
            }
            api.traffic.remove(worker_id);
            for version in versions {
                api.pins.retire_fence(version);
            }
            Ok(serde_json::json!({ "workerId": worker_id, "state": "tombstoned" }))
        },
    )
    .await;
    idempotent_response(response, StatusCode::ACCEPTED, request_id)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct VersionMetadata {
    main_module: String,
    #[serde(default)]
    vars: BTreeMap<String, serde_json::Value>,
    #[serde(default)]
    secrets: BTreeMap<String, SecretString>,
    #[serde(default)]
    bindings: BTreeMap<String, VersionBindingInput>,
    #[serde(default)]
    services: BTreeMap<String, VersionServiceInput>,
    #[serde(flatten)]
    runtime_features: VersionRuntimeFeatures,
    #[serde(default)]
    queue_consumers: Vec<QueueConsumerInput>,
    #[serde(default)]
    crons: Vec<String>,
    #[serde(default)]
    promote: bool,
}

async fn create_version(
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
    let metadata = match version_metadata(&request) {
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
            PlatformError::new(ErrorCode::BundleTooLarge, "version bundle exceeds limit"),
            request_id,
        );
    }
    let staging_admission = match api
        .storage
        .reserve_mutation(u64::try_from(body_limit).unwrap_or(u64::MAX))
    {
        Ok(value) => value,
        Err(error) => return error_response(error, request_id),
    };
    let staged = match stage_bundle(
        request.into_body(),
        api.storage.data_dir().version_staging_dir(),
        api.bundle_limits,
        body_limit,
    )
    .await
    {
        Ok(staged) => staged,
        Err(error) => return error_response(error, request_id),
    };
    drop(staging_admission);
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
    let mut controller = VersionController::new(
        &api.storage,
        api.artifacts.clone(),
        validator,
        api.bundle_limits,
    )
    .with_queue_consumer_limit(api.max_queue_consumer_concurrency);
    if let Some(promoter) = &api.product_promoter {
        controller = controller.with_product_promoter(promoter.clone());
    }
    let result = controller
        .create_version(CreateVersionRequest {
            account_id,
            worker_id,
            idempotency_key: key,
            content: VersionContent::Worker {
                bundle: VersionBundle::Staged(staged.bundle.clone()),
                assets: None,
            },
            vars: metadata.vars,
            secrets: metadata.secrets,
            bindings: metadata.bindings,
            services: metadata.services,
            runtime_features: metadata.runtime_features,
            queue_consumers: metadata.queue_consumers,
            crons: metadata.crons,
            promote: metadata.promote,
            request_id,
            now_ms: now_ms(),
        })
        .await;
    match result {
        Ok(CreateVersionOutcome::Applied(result)) => json_bytes(
            serde_json::to_vec(&serde_json::json!({
                "version": result.version.to_api_json(),
                "promoted": result.promoted,
            }))
            .unwrap_or_else(|_| b"{}".to_vec()),
            StatusCode::CREATED,
        ),
        Ok(CreateVersionOutcome::Replay(bytes)) => json_bytes(bytes, StatusCode::CREATED),
        Err(error) => error_response(error, request_id),
    }
}

async fn list_versions(
    State(state): State<HttpState>,
    Path((account, worker)): Path<(String, String)>,
    request: Request,
) -> Response {
    let request_id = request_id(&request);
    let Some(api) = authorized_api(&state, &request, request_id) else {
        return unauthorized_or_unavailable(&state, &request, request_id);
    };
    let result = parse_ids(&account, &worker).and_then(|(account_id, worker_id)| {
        WorkerRepository::new(api.storage.db()).list_versions(account_id, worker_id)
    });
    result_response(
        result.map(|versions| {
            serde_json::json!({
                "versions": versions.iter().map(version_json).collect::<Vec<_>>()
            })
        }),
        request_id,
    )
}

async fn get_version(
    State(state): State<HttpState>,
    Path((account, worker, version)): Path<(String, String, String)>,
    request: Request,
) -> Response {
    let request_id = request_id(&request);
    let Some(api) = authorized_api(&state, &request, request_id) else {
        return unauthorized_or_unavailable(&state, &request, request_id);
    };
    let result = parse_version_ids(&account, &worker, &version).and_then(
        |(account_id, worker_id, version_id)| {
            WorkerRepository::new(api.storage.db()).get_version(account_id, worker_id, version_id)
        },
    );
    result_response(
        result.map(|version| serde_json::json!({ "version": version_json(&version) })),
        request_id,
    )
}

async fn delete_version(
    State(state): State<HttpState>,
    Path((account, worker, version)): Path<(String, String, String)>,
    request: Request,
) -> Response {
    let request_id = request_id(&request);
    let Some(api) = authorized_api(&state, &request, request_id) else {
        return unauthorized_or_unavailable(&state, &request, request_id);
    };
    let (account_id, worker_id, version_id) = match parse_version_ids(&account, &worker, &version) {
        Ok(ids) => ids,
        Err(error) => return error_response(error, request_id),
    };
    let key = match idempotency_key(&request) {
        Ok(key) => key,
        Err(error) => return error_response(error, request_id),
    };
    let result = run_version_delete(api, account_id, worker_id, version_id, &key, request_id).await;
    idempotent_response(result, StatusCode::ACCEPTED, request_id)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PromotionBody {
    target_version_id: VersionId,
    #[serde(default)]
    expected_active_version_id: Option<VersionId>,
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
        "targetVersionId": body.target_version_id,
        "expectedActiveVersionId": body.expected_active_version_id,
    }))
    .unwrap_or_default();
    let scope = format!(
        "version.{}/{}",
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
        Some(body.target_version_id),
        || async {
            let _admission = api.storage.reserve_mutation(64 * 1024)?;
            let repo = WorkerRepository::new(api.storage.db());
            let worker_before = repo.get_tenant_worker(account_id, worker_id)?;
            if body.expected_active_version_id.is_some()
                && body.expected_active_version_id != worker_before.active_version_id
            {
                return Err(PlatformError::new(
                    ErrorCode::IdempotencyConflict,
                    "promotion compare-and-swap precondition failed",
                ));
            }
            let version = repo.get_version(account_id, worker_id, body.target_version_id)?;
            let reloads_durable_objects = BindingRepository::new(api.storage.db())
                .version_bindings(version.id)?
                .iter()
                .any(|binding| binding.kind == BindingKind::DoNamespace);
            for route in repo.list_routes(account_id, worker_id)? {
                if let Some(entrypoint) = route.entrypoint {
                    api.transport
                        .probe_entrypoint(
                            open_compute_workers::ValidationCandidate {
                                account_id,
                                worker_id,
                                version_id: version.id,
                                worker_code_sha256: version.worker_code_sha256,
                            },
                            entrypoint,
                        )
                        .await?;
                }
            }
            let promoted_at_ms = now_ms();
            if let Some(promoter) = &api.product_promoter {
                promoter
                    .promote(ProductPromotionRequest {
                        account_id,
                        worker_id,
                        version_id: body.target_version_id,
                        request_id,
                        now_ms: promoted_at_ms,
                    })
                    .await?;
            } else {
                repo.promote_checked(
                    account_id,
                    worker_id,
                    body.target_version_id,
                    body.expected_active_version_id,
                    Some(worker_before.route_generation),
                    request_id,
                    promoted_at_ms,
                )?;
            }
            let worker = repo.get_tenant_worker(account_id, worker_id)?;
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
            let _admission = api.storage.reserve_mutation(64 * 1024)?;
            let repo = WorkerRepository::new(api.storage.db());
            let expected_active = if let Some(entrypoint) = body.entrypoint.as_ref() {
                let worker = repo.get_tenant_worker(account_id, worker_id)?;
                let version_id = worker.active_version_id.ok_or_else(|| {
                    PlatformError::new(
                        ErrorCode::VersionNotReady,
                        "a named route requires an active version",
                    )
                })?;
                let version = repo.get_version(account_id, worker_id, version_id)?;
                api.transport
                    .probe_entrypoint(
                        open_compute_workers::ValidationCandidate {
                            account_id,
                            worker_id,
                            version_id,
                            worker_code_sha256: version.worker_code_sha256,
                        },
                        entrypoint.clone(),
                    )
                    .await?;
                Some(version_id)
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
                api.storage.hardening().max_routes_per_account,
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

/// Fallback for the public listener: resolve DB route, freeze version, stream through workerd.
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
    let pin = match api.pins.pin(snapshot.version.id) {
        Ok(pin) => pin,
        Err(error) => return error_response(error, request_id),
    };
    let target = DispatchTarget {
        account_id: snapshot.route.account_id,
        worker_id: snapshot.route.worker_id,
        version_id: snapshot.version.id,
        worker_code_sha256: hex::encode(snapshot.version.worker_code_sha256),
        entrypoint: snapshot.route.entrypoint,
        route_generation: i64::try_from(snapshot.worker.route_generation).unwrap_or(i64::MAX),
        request_id,
    };
    let worker_id = snapshot.route.worker_id;
    let started = std::time::Instant::now();
    let response = match api.transport.dispatch(target, request).await {
        Ok(response) => pin_response(response, pin),
        Err(error) => error_response(error, request_id),
    };
    api.traffic
        .observe(worker_id, response.status().as_u16(), started.elapsed());
    response
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
        error_response(
            PlatformError::new(ErrorCode::RuntimeUnavailable, "control plane is not ready"),
            request_id,
        )
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
                "failed to create version staging file",
            )
        })?;
    let cleanup = StagingCleanup { path: path.clone() };
    let mut file = tokio::fs::File::from_std(file);
    let mut written = 0_usize;
    while let Some(frame) = body.frame().await {
        let frame = frame.map_err(|_| {
            PlatformError::new(ErrorCode::BundleInvalid, "version upload stream failed")
        })?;
        let Ok(data) = frame.into_data() else {
            continue;
        };
        written = written.checked_add(data.len()).ok_or_else(|| {
            PlatformError::new(ErrorCode::BundleTooLarge, "version bundle exceeds limit")
        })?;
        if written > body_limit {
            return Err(PlatformError::new(
                ErrorCode::BundleTooLarge,
                "version bundle exceeds limit",
            ));
        }
        file.write_all(&data).await.map_err(|_| {
            PlatformError::new(
                ErrorCode::DiskHardLimit,
                "failed to write version staging file",
            )
        })?;
    }
    file.sync_all().await.map_err(|_| {
        PlatformError::new(
            ErrorCode::DiskHardLimit,
            "failed to persist version staging file",
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

fn version_metadata(request: &Request) -> Result<VersionMetadata, PlatformError> {
    let raw = request
        .headers()
        .get(VERSION_METADATA_HEADER)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| {
            PlatformError::new(
                ErrorCode::ConfigInvalid,
                "version metadata header is required",
            )
        })?;
    if raw.len() > MAX_VERSION_METADATA_HEADER_BYTES {
        return Err(PlatformError::new(
            ErrorCode::LimitInvalid,
            "version metadata header exceeds limit",
        ));
    }
    serde_json::from_str(raw)
        .map_err(|_| PlatformError::new(ErrorCode::ConfigInvalid, "version metadata is invalid"))
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

fn parse_version_ids(
    account: &str,
    worker: &str,
    version: &str,
) -> Result<(AccountId, WorkerId, VersionId), PlatformError> {
    let (account, worker) = parse_ids(account, worker)?;
    let version = VersionId::from_str(version)
        .map_err(|_| PlatformError::new(ErrorCode::ConfigInvalid, "version ID is invalid"))?;
    Ok((account, worker, version))
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
    version_ref: Option<VersionId>,
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
            if let Some(version_id) = version_ref {
                repo.complete_idempotency_with_version_ref(
                    account_id,
                    scope,
                    key,
                    &fingerprint,
                    &response,
                    version_id,
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
    version_ref: Option<VersionId>,
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
            if let Some(version_id) = version_ref {
                repo.complete_idempotency_with_version_ref(
                    account_id,
                    scope,
                    key,
                    &fingerprint,
                    &response,
                    version_id,
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

async fn run_version_delete(
    api: &WorkerApiState,
    account_id: AccountId,
    worker_id: WorkerId,
    version_id: VersionId,
    key: &str,
    request_id: RequestId,
) -> Result<Vec<u8>, PlatformError> {
    let scope = format!("version.delete/{worker_id}/{version_id}");
    let canonical = serde_json::to_vec(&serde_json::json!({
        "workerId": worker_id,
        "versionId": version_id,
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
        repo.begin_version_delete(account_id, worker_id, version_id)?;
        api.pins
            .fence_and_wait(version_id, api.delete_drain_timeout)
            .await?;
        repo.finalize_version_delete(account_id, worker_id, version_id, request_id, now_ms())?;
        serde_json::to_vec(&serde_json::json!({
            "versionId": version_id,
            "state": "tombstoned"
        }))
        .map_err(|_| internal())
    }
    .await;
    match operation {
        Ok(response) => {
            api.pins.retire_fence(version_id);
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
    PlatformError::new(
        ErrorCode::from_stable_str(&code).unwrap_or(ErrorCode::Internal),
        "idempotent operation previously failed",
    )
}

fn idempotency_ref_id(account_id: AccountId, scope: &str, key: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"open-compute/version-referrer/v1\0");
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

fn version_json(version: &VersionRecord) -> serde_json::Value {
    version.to_api_json()
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
        | ErrorCode::VersionNotFound
        | ErrorCode::RouteNotFound
        | ErrorCode::EntrypointNotFound => StatusCode::NOT_FOUND,
        ErrorCode::WorkerNameConflict | ErrorCode::RouteConflict => StatusCode::CONFLICT,
        ErrorCode::VersionNotReady
        | ErrorCode::VersionActive
        | ErrorCode::VersionReferenced
        | ErrorCode::ServiceTargetReferenced
        | ErrorCode::IdempotencyConflict
        | ErrorCode::AssetUploadIncomplete
        | ErrorCode::AssetUploadConflict => StatusCode::CONFLICT,
        ErrorCode::BundleTooLarge | ErrorCode::LimitInvalid | ErrorCode::AssetLimitExceeded => {
            StatusCode::PAYLOAD_TOO_LARGE
        }
        ErrorCode::BundleRuntimeInvalid
        | ErrorCode::CompatibilityUnsupported
        | ErrorCode::AssetConfigUnsupported => StatusCode::UNPROCESSABLE_ENTITY,
        ErrorCode::RuntimeUnavailable
        | ErrorCode::ArtifactUnavailable
        | ErrorCode::AssetStorageUnavailable => StatusCode::SERVICE_UNAVAILABLE,
        ErrorCode::ResourceLimitExceeded | ErrorCode::QuotaExceeded | ErrorCode::AdmissionBusy => {
            StatusCode::TOO_MANY_REQUESTS
        }
        ErrorCode::StoragePressure | ErrorCode::DiskHardLimit => StatusCode::INSUFFICIENT_STORAGE,
        ErrorCode::PlatformUnavailable => StatusCode::SERVICE_UNAVAILABLE,
        ErrorCode::Internal
        | ErrorCode::RuntimeResultUnknown
        | ErrorCode::VersionInvariantViolation
        | ErrorCode::ArtifactIntegrityError
        | ErrorCode::AssetIntegrityError => StatusCode::INTERNAL_SERVER_ERROR,
        _ => StatusCode::BAD_REQUEST,
    };
    let mut response = (
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
        .into_response();
    response
        .extensions_mut()
        .insert(ProductErrorCode(error.code()));
    response
}

fn internal() -> PlatformError {
    PlatformError::new(ErrorCode::Internal, "internal control operation failed")
}

#[cfg(test)]
#[path = "workers_http_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "workers_http_asset_tests.rs"]
mod asset_tests;
