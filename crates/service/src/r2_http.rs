//! P0.5 logical R2 bucket control API and lifecycle recovery.

use crate::http::{HttpState, ProductErrorCode, authorize};
use crate::r2_backend::R2BindingService;
use axum::Router;
use axum::body::{Body, to_bytes};
use axum::extract::{Path, Query, Request, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use open_compute_artifacts::{R2_MAX_LIST_LIMIT, R2ObjectStore, UserObjectKey};
use open_compute_core::{
    AccountId, BindingKind, ErrorCode, PlatformError, R2Config, RequestId, ResourceId,
    ResourceState,
};
use open_compute_storage::{
    CatalogDirection, CatalogSort, DEFAULT_CATALOG_LIST_LIMIT, PlatformStorage, R2_SCHEMA_VERSION,
    R2BucketRecord, R2BucketRepository, R2ObjectListEntry, R2ObjectRepository,
    ReserveResourceCreate, ReserveResourceDelete, ResourceCreateReservation,
    ResourceDeleteReservation, ResourceRepository, decode_catalog_cursor, normalize_catalog_limit,
};
use open_compute_workers::{R2ResourceDriver, ResourcePins};
use serde::{Deserialize, Serialize};
use std::str::FromStr as _;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

const MAX_JSON_BODY: usize = 4096;
const IDEMPOTENCY_HEADER: &str = "idempotency-key";
const IDEMPOTENCY_TTL_MS: i64 = 24 * 60 * 60 * 1000;

/// Shared logical-bucket control-plane composition state.
#[derive(Clone)]
pub struct R2ApiState {
    storage: Arc<PlatformStorage>,
    objects: R2ObjectStore,
    pins: ResourcePins,
    config: R2Config,
    delete_drain_timeout: Duration,
    force_deletes: Arc<Semaphore>,
    metrics: Option<Arc<crate::metrics::MetricsRegistry>>,
    binding: Option<Arc<R2BindingService>>,
}

impl std::fmt::Debug for R2ApiState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("R2ApiState")
            .field("config", &self.config)
            .field("binding", &self.binding.is_some())
            .finish_non_exhaustive()
    }
}

impl R2ApiState {
    /// Bind durable authority, typed S3 access, pins, and frozen defaults.
    #[must_use]
    pub fn new(
        storage: Arc<PlatformStorage>,
        objects: R2ObjectStore,
        pins: ResourcePins,
        config: R2Config,
        delete_drain_timeout: Duration,
    ) -> Self {
        Self {
            storage,
            objects,
            pins,
            config,
            delete_drain_timeout,
            force_deletes: Arc::new(Semaphore::new(1)),
            metrics: None,
            binding: None,
        }
    }

    /// Attach the authoritative R2 binding executor used for item-level operator APIs.
    #[must_use]
    pub fn with_binding(mut self, binding: Arc<R2BindingService>) -> Self {
        self.binding = Some(binding);
        self
    }

    pub(crate) fn binding(&self) -> Result<&Arc<R2BindingService>, PlatformError> {
        self.binding.as_ref().ok_or_else(|| {
            PlatformError::new(
                ErrorCode::PlatformUnavailable,
                "R2 operator binding is unavailable",
            )
        })
    }

    /// Attach fixed-series observability for force-delete progress.
    #[must_use]
    pub fn with_metrics(mut self, metrics: Arc<crate::metrics::MetricsRegistry>) -> Self {
        self.metrics = Some(metrics);
        self
    }

    /// Recover every creating/deleting R2 lifecycle before readiness.
    pub async fn reconcile_pending(&self) -> Result<u32, PlatformError> {
        let candidates = ResourceRepository::new(self.storage.db()).reconcile_candidates()?;
        let driver = self.driver();
        let mut reconciled = 0_u32;
        for resource in candidates {
            if resource.kind != BindingKind::R2Bucket {
                continue;
            }
            match resource.state {
                ResourceState::Creating => {
                    driver.reconcile(&resource).await?;
                    ResourceRepository::new(self.storage.db()).mark_ready(resource.id, now_ms())?;
                }
                ResourceState::Deleting => {
                    let bucket = R2BucketRepository::new(self.storage.db())
                        .get(resource.account_id, resource.id)?;
                    R2BucketRepository::new(self.storage.db())
                        .mark_delete_started(resource.id, now_ms())?;
                    crate::r2_backend::multipart::reconcile_bucket_multipart(
                        &self.storage,
                        &self.objects,
                        &bucket,
                        true,
                        true,
                        Duration::from_millis(self.config.operation_timeout_ms),
                    )
                    .await?;
                    crate::r2_backend::objects::reconcile_bucket_objects(
                        &self.storage,
                        &self.objects,
                        &bucket,
                        Duration::from_millis(self.config.operation_timeout_ms),
                    )
                    .await?;
                    driver.drain_objects(&bucket).await?;
                    driver.finalize_delete(&bucket).await?;
                    ResourceRepository::new(self.storage.db()).mark_tombstoned(
                        resource.account_id,
                        resource.id,
                        RequestId::generate(),
                        now_ms(),
                    )?;
                }
                ResourceState::Ready | ResourceState::Tombstoned => continue,
            }
            reconciled = reconciled.saturating_add(1);
        }
        for bucket in R2BucketRepository::new(self.storage.db()).list_all()? {
            if bucket.resource.state == ResourceState::Ready {
                crate::r2_backend::multipart::reconcile_bucket_multipart(
                    &self.storage,
                    &self.objects,
                    &bucket,
                    true,
                    false,
                    Duration::from_millis(self.config.operation_timeout_ms),
                )
                .await?;
                crate::r2_backend::objects::reconcile_bucket_objects(
                    &self.storage,
                    &self.objects,
                    &bucket,
                    Duration::from_millis(self.config.operation_timeout_ms),
                )
                .await?;
            }
        }
        Ok(reconciled)
    }

    fn driver(&self) -> R2ResourceDriver<'_> {
        R2ResourceDriver::new(&self.storage, self.objects.clone(), self.config.clone())
    }

    pub(crate) fn storage(&self) -> &Arc<PlatformStorage> {
        &self.storage
    }

    pub(crate) const fn objects(&self) -> &R2ObjectStore {
        &self.objects
    }

    pub(crate) fn pins(&self) -> &ResourcePins {
        &self.pins
    }

    pub(crate) const fn config(&self) -> &R2Config {
        &self.config
    }

    pub(crate) const fn delete_drain_timeout(&self) -> Duration {
        self.delete_drain_timeout
    }

    pub(crate) fn resource_driver(&self) -> R2ResourceDriver<'_> {
        self.driver()
    }
}

struct ForceDeleteGuard {
    _permit: OwnedSemaphorePermit,
    metrics: Option<Arc<crate::metrics::MetricsRegistry>>,
}

impl ForceDeleteGuard {
    async fn acquire(api: &R2ApiState) -> Result<Self, PlatformError> {
        let permit = tokio::time::timeout(
            api.delete_drain_timeout,
            api.force_deletes.clone().acquire_owned(),
        )
        .await
        .map_err(|_| overloaded())?
        .map_err(|_| overloaded())?;
        if let Some(metrics) = &api.metrics {
            metrics.set_r2_force_delete_remaining_batches(1);
        }
        Ok(Self {
            _permit: permit,
            metrics: api.metrics.clone(),
        })
    }
}

impl Drop for ForceDeleteGuard {
    fn drop(&mut self) {
        if let Some(metrics) = &self.metrics {
            metrics.set_r2_force_delete_remaining_batches(0);
        }
    }
}

/// Router for logical R2 bucket management and bounded operator object APIs.
pub fn control_router() -> Router<HttpState> {
    Router::new()
        .route(
            "/v1/accounts/{account_id}/r2/buckets",
            post(create_bucket).get(list_buckets),
        )
        .route(
            "/v1/accounts/{account_id}/r2/buckets/{resource_id}",
            get(get_bucket).patch(rename_bucket).delete(delete_bucket),
        )
        .route(
            "/v1/accounts/{account_id}/r2/buckets/{resource_id}/objects",
            get(list_objects),
        )
        .route(
            "/v1/accounts/{account_id}/r2/buckets/{resource_id}/objects/{key}",
            get(get_object).put(put_object).delete(delete_object),
        )
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ListBucketsQuery {
    search: Option<String>,
    status: Option<ResourceState>,
    sort: Option<CatalogSort>,
    direction: Option<CatalogDirection>,
    cursor: Option<String>,
    limit: Option<u16>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ListObjectsQuery {
    prefix: Option<String>,
    cursor: Option<String>,
    limit: Option<u16>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateBucketBody {
    name: String,
}

async fn create_bucket(
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
    let body = match read_json::<CreateBucketBody>(request).await {
        Ok(value) => value,
        Err(error) => return error_response(error, request_id),
    };
    match create(api, account_id, body.name, key, request_id).await {
        Ok(CreateOutcome::Applied(bucket)) => json_response(
            &serde_json::json!({ "bucket": BucketView::from(bucket.as_ref()) }),
            StatusCode::CREATED,
        ),
        Ok(CreateOutcome::Replay(bytes)) => json_bytes(bytes, StatusCode::OK),
        Err(error) => error_response(error, request_id),
    }
}

enum CreateOutcome {
    Applied(Box<R2BucketRecord>),
    Replay(Vec<u8>),
}

async fn create(
    api: &R2ApiState,
    account_id: AccountId,
    name: String,
    key: String,
    request_id: RequestId,
) -> Result<CreateOutcome, PlatformError> {
    let _admission = api.storage.reserve_mutation(64 * 1024)?;
    let fingerprint_input = serde_json::to_vec(&serde_json::json!({
        "v": 1,
        "accountId": account_id,
        "kind": BindingKind::R2Bucket,
        "name": name,
        "maxObjectBytes": api.config.max_object_bytes,
    }))
    .map_err(|_| internal())?;
    let fingerprint = api.storage.crypto().fingerprint_request(&fingerprint_input);
    let now = now_ms();
    let reservation = ResourceRepository::new(api.storage.db()).reserve_create(
        &ReserveResourceCreate {
            account_id,
            kind: BindingKind::R2Bucket,
            name: &name,
            idempotency_key: &key,
            fingerprint_key_id: api.storage.crypto().fingerprint_key_id(),
            request_fingerprint: &fingerprint,
            resource_id: ResourceId::generate(),
            driver_schema_version: R2_SCHEMA_VERSION,
            request_id,
            now_ms: now,
            expires_at_ms: now.saturating_add(IDEMPOTENCY_TTL_MS),
        },
        api.storage.hardening().max_resources_per_kind_per_account,
    )?;
    let resource = match reservation {
        ResourceCreateReservation::Complete(bytes) => return Ok(CreateOutcome::Replay(bytes)),
        ResourceCreateReservation::Failed(_) => {
            return Err(PlatformError::new(
                ErrorCode::ResourceInvariantViolation,
                "R2 bucket create previously failed",
            ));
        }
        ResourceCreateReservation::Reserved(resource)
        | ResourceCreateReservation::Continue(resource) => resource,
    };
    let bucket = api.driver().reconcile(&resource).await?;
    if resource.state == ResourceState::Creating {
        ResourceRepository::new(api.storage.db()).mark_ready(resource.id, now_ms())?;
    }
    let bucket = R2BucketRepository::new(api.storage.db()).get(account_id, bucket.resource.id)?;
    let response = serde_json::to_vec(&serde_json::json!({
        "bucket": BucketView::from(&bucket),
    }))
    .map_err(|_| internal())?;
    ResourceRepository::new(api.storage.db()).complete_create(
        account_id,
        &key,
        &fingerprint,
        bucket.resource.id,
        &response,
    )?;
    Ok(CreateOutcome::Applied(Box::new(bucket)))
}

async fn list_buckets(
    State(state): State<HttpState>,
    Path(account): Path<String>,
    query: Result<Query<ListBucketsQuery>, axum::extract::rejection::QueryRejection>,
    request: Request,
) -> Response {
    let request_id = request_id(&request);
    let Some(api) = authorized_api(&state, &request) else {
        return unauthorized_or_unavailable(&state, &request, request_id);
    };
    let Ok(Query(query)) = query else {
        return error_response(
            PlatformError::new(ErrorCode::ConfigInvalid, "R2 bucket list query is invalid"),
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
    let sort = query.sort.unwrap_or(CatalogSort::Name);
    let direction = query.direction.unwrap_or(CatalogDirection::Asc);
    let search = query
        .search
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let storage = api.storage.clone();
    match tokio::task::spawn_blocking(move || {
        R2BucketRepository::new(storage.db()).list_page(
            account_id,
            search.as_deref(),
            query.status,
            sort,
            direction,
            after,
            limit,
        )
    })
    .await
    {
        Ok(Ok(page)) => json_response(
            &serde_json::json!({
                "buckets": page.items.iter().map(BucketView::from).collect::<Vec<_>>(),
                "cursor": page.next_cursor,
                "listComplete": page.next_cursor.is_none(),
            }),
            StatusCode::OK,
        ),
        Ok(Err(error)) => error_response(error, request_id),
        Err(_) => error_response(internal(), request_id),
    }
}

async fn get_bucket(
    State(state): State<HttpState>,
    Path((account, resource)): Path<(String, String)>,
    request: Request,
) -> Response {
    let request_id = request_id(&request);
    let Some(api) = authorized_api(&state, &request) else {
        return unauthorized_or_unavailable(&state, &request, request_id);
    };
    let (account_id, resource_id) = match parse_ids(&account, &resource) {
        Ok(value) => value,
        Err(error) => return error_response(error, request_id),
    };
    match R2BucketRepository::new(api.storage.db()).get(account_id, resource_id) {
        Ok(bucket) => json_response(
            &serde_json::json!({ "bucket": BucketView::from(&bucket) }),
            StatusCode::OK,
        ),
        Err(error) => error_response(error, request_id),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RenameBucketBody {
    name: String,
}

async fn rename_bucket(
    State(state): State<HttpState>,
    Path((account, resource)): Path<(String, String)>,
    request: Request,
) -> Response {
    let request_id = request_id(&request);
    let Some(api) = authorized_api(&state, &request) else {
        return unauthorized_or_unavailable(&state, &request, request_id);
    };
    let (account_id, resource_id) = match parse_ids(&account, &resource) {
        Ok(value) => value,
        Err(error) => return error_response(error, request_id),
    };
    let body = match read_json::<RenameBucketBody>(request).await {
        Ok(value) => value,
        Err(error) => return error_response(error, request_id),
    };
    let _admission = match api.storage.reserve_mutation(64 * 1024) {
        Ok(value) => value,
        Err(error) => return error_response(error, request_id),
    };
    match ResourceRepository::new(api.storage.db()).get(account_id, resource_id) {
        Ok(resource) if resource.kind == BindingKind::R2Bucket => {}
        Ok(_) => return error_response(not_found(), request_id),
        Err(error) => return error_response(error, request_id),
    }
    match ResourceRepository::new(api.storage.db()).rename(
        account_id,
        resource_id,
        &body.name,
        request_id,
        now_ms(),
    ) {
        Ok(_) => match R2BucketRepository::new(api.storage.db()).get(account_id, resource_id) {
            Ok(bucket) => json_response(
                &serde_json::json!({ "bucket": BucketView::from(&bucket) }),
                StatusCode::OK,
            ),
            Err(error) => error_response(error, request_id),
        },
        Err(error) => error_response(error, request_id),
    }
}

async fn delete_bucket(
    State(state): State<HttpState>,
    Path((account, resource)): Path<(String, String)>,
    request: Request,
) -> Response {
    let request_id = request_id(&request);
    let force = match parse_force(request.uri().query()) {
        Ok(force) => force,
        Err(error) => return error_response(error, request_id),
    };
    let Some(api) = authorized_api(&state, &request) else {
        return unauthorized_or_unavailable(&state, &request, request_id);
    };
    let (account_id, resource_id) = match parse_ids(&account, &resource) {
        Ok(value) => value,
        Err(error) => return error_response(error, request_id),
    };
    let key = match idempotency_key(&request) {
        Ok(key) => key,
        Err(error) => return error_response(error, request_id),
    };
    match delete(api, account_id, resource_id, force, &key, request_id).await {
        Ok(DeleteOutcome::Applied(bytes)) => json_bytes(bytes, StatusCode::ACCEPTED),
        Ok(DeleteOutcome::Replay(bytes)) => json_bytes(bytes, StatusCode::OK),
        Err(error) => error_response(error, request_id),
    }
}

enum DeleteOutcome {
    Applied(Vec<u8>),
    Replay(Vec<u8>),
}

async fn delete(
    api: &R2ApiState,
    account_id: AccountId,
    resource_id: ResourceId,
    force: bool,
    key: &str,
    request_id: RequestId,
) -> Result<DeleteOutcome, PlatformError> {
    let resources = ResourceRepository::new(api.storage.db());
    let fingerprint_input = serde_json::to_vec(&serde_json::json!({
        "v": 1,
        "accountId": account_id,
        "resourceId": resource_id,
        "force": force,
    }))
    .map_err(|_| internal())?;
    let fingerprint = api.storage.crypto().fingerprint_request(&fingerprint_input);
    let now = now_ms();
    let reservation = resources.reserve_delete(&ReserveResourceDelete {
        account_id,
        resource_id,
        idempotency_key: key,
        fingerprint_key_id: api.storage.crypto().fingerprint_key_id(),
        request_fingerprint: &fingerprint,
        now_ms: now,
        expires_at_ms: now.saturating_add(IDEMPOTENCY_TTL_MS),
    })?;
    let resource = match reservation {
        ResourceDeleteReservation::Complete(bytes) => return Ok(DeleteOutcome::Replay(bytes)),
        ResourceDeleteReservation::Failed(_) => {
            return Err(PlatformError::new(
                ErrorCode::ResourceInvariantViolation,
                "R2 bucket delete previously failed",
            ));
        }
        ResourceDeleteReservation::Reserved(resource)
        | ResourceDeleteReservation::Continue(resource) => resource,
    };
    if resource.kind != BindingKind::R2Bucket {
        return Err(not_found());
    }
    if resource.state != ResourceState::Tombstoned {
        if !resources.referrers(resource_id)?.is_empty() {
            return Err(PlatformError::new(
                ErrorCode::ResourceReferenced,
                "R2 bucket still has retained referrers",
            ));
        }
        let bucket = R2BucketRepository::new(api.storage.db()).get(account_id, resource_id)?;
        let driver = api.driver();
        if !force && resource.state != ResourceState::Deleting {
            driver.require_empty(&bucket).await?;
        }
        let _force_delete = if force || resource.state == ResourceState::Deleting {
            Some(ForceDeleteGuard::acquire(api).await?)
        } else {
            None
        };
        api.pins
            .fence_and_wait(resource_id, api.delete_drain_timeout)
            .await?;
        let result = async {
            resources.begin_delete(account_id, resource_id, now_ms())?;
            R2BucketRepository::new(api.storage.db()).mark_delete_started(resource_id, now_ms())?;
            crate::r2_backend::multipart::reconcile_bucket_multipart(
                &api.storage,
                &api.objects,
                &bucket,
                false,
                true,
                Duration::from_millis(api.config.operation_timeout_ms),
            )
            .await?;
            crate::r2_backend::objects::reconcile_bucket_objects(
                &api.storage,
                &api.objects,
                &bucket,
                Duration::from_millis(api.config.operation_timeout_ms),
            )
            .await?;
            if force || resource.state == ResourceState::Deleting {
                driver.drain_objects(&bucket).await?;
            }
            driver.finalize_delete(&bucket).await?;
            resources.mark_tombstoned(account_id, resource_id, request_id, now_ms())
        }
        .await;
        if result.is_ok() {
            api.pins.retire_fence(resource_id);
        } else {
            api.pins.unfence(resource_id);
        }
        result?;
    }
    let response = serde_json::to_vec(
        &serde_json::json!({ "resourceId": resource_id, "state": "tombstoned" }),
    )
    .map_err(|_| internal())?;
    resources.complete_delete(account_id, key, &fingerprint, resource_id, &response)?;
    Ok(DeleteOutcome::Applied(response))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct BucketView<'a> {
    resource_id: ResourceId,
    name: &'a str,
    state: ResourceState,
    availability: open_compute_core::ResourceAvailability,
    created_at_ms: i64,
    updated_at_ms: i64,
    max_object_bytes: u64,
}

impl<'a> From<&'a R2BucketRecord> for BucketView<'a> {
    fn from(bucket: &'a R2BucketRecord) -> Self {
        Self {
            resource_id: bucket.resource.id,
            name: &bucket.resource.name,
            state: bucket.resource.state,
            availability: bucket.resource.availability,
            created_at_ms: bucket.resource.created_at_ms,
            updated_at_ms: bucket.resource.updated_at_ms,
            max_object_bytes: bucket.max_object_bytes,
        }
    }
}

async fn get_object(
    State(state): State<HttpState>,
    Path((account, resource, key)): Path<(String, String, String)>,
    request: Request,
) -> Response {
    let request_id = request_id(&request);
    let authorized = authorize(&state, &request);
    let metadata = request
        .uri()
        .query()
        .is_some_and(|query| query.split('&').any(|part| part == "metadata=true"));
    if metadata {
        return object_metadata_response(
            &state,
            request_id,
            authorized,
            account.as_str(),
            resource.as_str(),
            key.as_str(),
        )
        .await;
    }
    if !authorized {
        return unauthorized_or_unavailable_authed(&state, request_id, false);
    }
    let (account_id, resource_id) = match parse_ids(account.as_str(), resource.as_str()) {
        Ok(value) => value,
        Err(error) => return error_response(error, request_id),
    };
    let object_key = match parse_object_key(key.as_str()) {
        Ok(value) => value,
        Err(error) => return error_response(error, request_id),
    };
    let Some(api) = state.r2_api() else {
        return error_response(
            PlatformError::new(
                ErrorCode::PlatformUnavailable,
                "R2 operator API is unavailable",
            ),
            request_id,
        );
    };
    let binding = match api.binding() {
        Ok(value) => value.clone(),
        Err(error) => return error_response(error, request_id),
    };
    match binding
        .operator_object_get(account_id, resource_id, &object_key)
        .await
    {
        Ok(Some((metadata, body))) => {
            let mut response = Response::new(Body::from(body));
            *response.status_mut() = StatusCode::OK;
            response.headers_mut().insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/octet-stream"),
            );
            if let Ok(value) = HeaderValue::from_str(&metadata.etag) {
                response.headers_mut().insert(header::ETAG, value);
            }
            response
        }
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(error) => error_response(error, request_id),
    }
}

async fn put_object(
    State(state): State<HttpState>,
    Path((account, resource, key)): Path<(String, String, String)>,
    request: Request,
) -> Response {
    let request_id = request_id(&request);
    let Some(api) = authorized_api(&state, &request) else {
        return unauthorized_or_unavailable(&state, &request, request_id);
    };
    let (account_id, resource_id) = match parse_ids(&account, &resource) {
        Ok(value) => value,
        Err(error) => return error_response(error, request_id),
    };
    let object_key = match parse_object_key(&key) {
        Ok(value) => value,
        Err(error) => return error_response(error, request_id),
    };
    if let Err(error) = idempotency_key(&request) {
        return error_response(error, request_id);
    }
    let binding = match api.binding() {
        Ok(value) => value.clone(),
        Err(error) => return error_response(error, request_id),
    };
    let body = request.into_body();
    match binding
        .operator_object_put(
            account_id,
            resource_id,
            &object_key,
            request_id,
            open_compute_artifacts::R2PutOptions::default(),
            None,
            body,
        )
        .await
    {
        Ok(Some(metadata)) => json_response(
            &serde_json::json!({
                "key": metadata.key,
                "size": metadata.size,
                "etag": metadata.etag,
                "uploaded": metadata.uploaded,
            }),
            StatusCode::OK,
        ),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(error) => error_response(error, request_id),
    }
}

async fn delete_object(
    State(state): State<HttpState>,
    Path((account, resource, key)): Path<(String, String, String)>,
    request: Request,
) -> Response {
    let request_id = request_id(&request);
    let Some(api) = authorized_api(&state, &request) else {
        return unauthorized_or_unavailable(&state, &request, request_id);
    };
    let (account_id, resource_id) = match parse_ids(&account, &resource) {
        Ok(value) => value,
        Err(error) => return error_response(error, request_id),
    };
    let object_key = match parse_object_key(&key) {
        Ok(value) => value,
        Err(error) => return error_response(error, request_id),
    };
    if let Err(error) = idempotency_key(&request) {
        return error_response(error, request_id);
    }
    let binding = match api.binding() {
        Ok(value) => value.clone(),
        Err(error) => return error_response(error, request_id),
    };
    match binding
        .operator_object_delete(account_id, resource_id, &object_key)
        .await
    {
        Ok(true) => json_response(
            &serde_json::json!({ "key": object_key.as_str() }),
            StatusCode::OK,
        ),
        Ok(false) => StatusCode::NOT_FOUND.into_response(),
        Err(error) => error_response(error, request_id),
    }
}

async fn object_metadata_response(
    state: &HttpState,
    request_id: RequestId,
    authorized: bool,
    account: &str,
    resource: &str,
    key: &str,
) -> Response {
    if !authorized {
        return unauthorized_or_unavailable_authed(state, request_id, false);
    }
    let Some(api) = state.r2_api() else {
        return unauthorized_or_unavailable_authed(state, request_id, true);
    };
    let (account_id, resource_id) = match parse_ids(account, resource) {
        Ok(value) => value,
        Err(error) => return error_response(error, request_id),
    };
    let object_key = match parse_object_key(key) {
        Ok(value) => value,
        Err(error) => return error_response(error, request_id),
    };
    let binding = match api.binding() {
        Ok(value) => value.clone(),
        Err(error) => return error_response(error, request_id),
    };
    match binding
        .operator_object_head(account_id, resource_id, &object_key)
        .await
    {
        Ok(Some(metadata)) => json_response(
            &serde_json::json!({
                "key": metadata.key,
                "size": metadata.size,
                "etag": metadata.etag,
                "uploaded": metadata.uploaded,
            }),
            StatusCode::OK,
        ),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(error) => error_response(error, request_id),
    }
}

fn unauthorized_or_unavailable_authed(
    _state: &HttpState,
    request_id: RequestId,
    authorized: bool,
) -> Response {
    if !authorized {
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

fn parse_object_key(value: &str) -> Result<UserObjectKey, PlatformError> {
    if value.is_empty() {
        return Err(PlatformError::new(
            ErrorCode::ConfigInvalid,
            "R2 object key is invalid",
        ));
    }
    UserObjectKey::parse(value)
}

async fn list_objects(
    State(state): State<HttpState>,
    Path((account, resource)): Path<(String, String)>,
    query: Result<Query<ListObjectsQuery>, axum::extract::rejection::QueryRejection>,
    request: Request,
) -> Response {
    let request_id = request_id(&request);
    let Some(api) = authorized_api(&state, &request) else {
        return unauthorized_or_unavailable(&state, &request, request_id);
    };
    let Ok(Query(query)) = query else {
        return error_response(
            PlatformError::new(ErrorCode::ConfigInvalid, "R2 object list query is invalid"),
            request_id,
        );
    };
    let (account_id, resource_id) = match parse_ids(&account, &resource) {
        Ok(value) => value,
        Err(error) => return error_response(error, request_id),
    };
    let prefix = query.prefix.unwrap_or_default();
    let limit = query.limit.unwrap_or(100).clamp(1, R2_MAX_LIST_LIMIT);
    let after = query.cursor;
    let storage = api.storage.clone();
    let objects = api.objects.clone();
    let page = match tokio::task::spawn_blocking(move || {
        let bucket = R2BucketRepository::new(storage.db()).get(account_id, resource_id)?;
        let locator = objects.locator(bucket.resource.id, &bucket.physical_prefix)?;
        let page = R2ObjectRepository::new(storage.db()).list(
            account_id,
            resource_id,
            &prefix,
            None,
            after.as_deref(),
            limit,
        )?;
        Ok::<_, PlatformError>((bucket, locator, page))
    })
    .await
    {
        Ok(Ok(value)) => value,
        Ok(Err(error)) => return error_response(error, request_id),
        Err(_) => return error_response(internal(), request_id),
    };
    let (bucket, locator, page) = page;
    let _ = bucket;
    let mut listed = Vec::new();
    for entry in page.entries {
        let R2ObjectListEntry::Object(record) = entry else {
            continue;
        };
        let key = match UserObjectKey::parse(&record.object_key) {
            Ok(value) => value,
            Err(error) => return error_response(error, request_id),
        };
        let ssec = match crate::r2_backend::objects::open_object_ssec(&api.storage, &record) {
            Ok(value) => value,
            Err(error) => return error_response(error, request_id),
        };
        match api.objects.head(&locator, &key, ssec.as_ref()).await {
            Ok(Some(metadata)) => listed.push(serde_json::json!({
                "key": metadata.key,
                "size": metadata.size,
                "etag": metadata.etag,
                "uploaded": metadata.uploaded,
            })),
            Ok(None) => {}
            Err(error) => return error_response(error, request_id),
        }
    }
    json_response(
        &serde_json::json!({
            "objects": listed,
            "cursor": page.next_after,
            "truncated": page.next_after.is_some(),
        }),
        StatusCode::OK,
    )
}

fn authorized_api<'a>(state: &'a HttpState, request: &Request) -> Option<&'a Arc<R2ApiState>> {
    authorize(state, request).then(|| state.r2_api()).flatten()
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

async fn read_json<T: for<'de> Deserialize<'de>>(request: Request) -> Result<T, PlatformError> {
    let bytes = to_bytes(request.into_body(), MAX_JSON_BODY)
        .await
        .map_err(|_| {
            PlatformError::new(
                ErrorCode::LimitInvalid,
                "control request body exceeds limit",
            )
        })?;
    serde_json::from_slice(&bytes).map_err(|_| {
        PlatformError::new(ErrorCode::ConfigInvalid, "control request JSON is invalid")
    })
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

fn parse_force(query: Option<&str>) -> Result<bool, PlatformError> {
    match query {
        None | Some("") | Some("force=false") => Ok(false),
        Some("force=true") => Ok(true),
        Some(_) => Err(PlatformError::new(
            ErrorCode::ConfigInvalid,
            "R2 delete query is invalid",
        )),
    }
}

fn parse_account(value: &str) -> Result<AccountId, PlatformError> {
    AccountId::from_str(value)
        .map_err(|_| PlatformError::new(ErrorCode::ConfigInvalid, "account ID is invalid"))
}

fn parse_ids(account: &str, resource: &str) -> Result<(AccountId, ResourceId), PlatformError> {
    Ok((
        parse_account(account)?,
        ResourceId::from_str(resource)
            .map_err(|_| PlatformError::new(ErrorCode::ConfigInvalid, "resource ID is invalid"))?,
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
        [(header::CONTENT_TYPE, "application/json")],
        Body::from(bytes),
    )
        .into_response()
}

fn error_response(
    error: impl std::borrow::Borrow<PlatformError>,
    request_id: RequestId,
) -> Response {
    let error = error.borrow();
    let code = error.code();
    let status = match code {
        ErrorCode::ResourceNotFound => StatusCode::NOT_FOUND,
        ErrorCode::ResourceNameConflict
        | ErrorCode::IdempotencyConflict
        | ErrorCode::ResourceReferenced
        | ErrorCode::ResourceNotReady
        | ErrorCode::R2BucketNotEmpty => StatusCode::CONFLICT,
        ErrorCode::AdminAuthRequired => StatusCode::UNAUTHORIZED,
        ErrorCode::ConfigInvalid | ErrorCode::LimitInvalid => StatusCode::BAD_REQUEST,
        ErrorCode::QuotaExceeded | ErrorCode::AdmissionBusy => StatusCode::TOO_MANY_REQUESTS,
        ErrorCode::StoragePressure | ErrorCode::DiskHardLimit => StatusCode::INSUFFICIENT_STORAGE,
        ErrorCode::PlatformUnavailable => StatusCode::SERVICE_UNAVAILABLE,
        ErrorCode::R2ProviderUnavailable | ErrorCode::R2Overloaded | ErrorCode::R2ResultUnknown => {
            StatusCode::SERVICE_UNAVAILABLE
        }
        ErrorCode::R2PrefixCollision | ErrorCode::ResourceInvariantViolation => {
            StatusCode::UNPROCESSABLE_ENTITY
        }
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    };
    let mut response = (status, axum::Json(serde_json::json!({"ok": false, "error": {"code": code.as_str(), "message": "control request failed", "requestId": request_id}}))).into_response();
    response.extensions_mut().insert(ProductErrorCode(code));
    response
}

fn not_found() -> PlatformError {
    PlatformError::new(ErrorCode::ResourceNotFound, "R2 bucket was not found")
}
fn internal() -> PlatformError {
    PlatformError::new(ErrorCode::Internal, "R2 control operation failed")
}

fn overloaded() -> PlatformError {
    PlatformError::new(
        ErrorCode::R2Overloaded,
        "R2 force-delete capacity is temporarily saturated",
    )
}

#[cfg(test)]
#[path = "r2_http_tests.rs"]
mod tests;
