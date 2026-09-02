//! P0.6 D1 database control API.

use crate::d1_backend::D1BindingService;
use crate::http::{HttpState, ProductErrorCode, authorize};
use crate::metrics::{D1Lifecycle, D1LifecycleGuard};
use axum::Router;
use axum::body::{Body, to_bytes};
use axum::extract::{Path, Query, Request, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use open_compute_artifacts::ArtifactStore;
use open_compute_core::{
    AccountId, BindingKind, D1Config, ErrorCode, PlatformError, RequestId, ResourceId,
    ResourceState,
};
use open_compute_storage::{
    CatalogDirection, CatalogSort, D1_DATABASE_SCHEMA_VERSION, D1BackupState, D1DatabaseRepository,
    D1Engine, D1Migration, D1Paths, DEFAULT_CATALOG_LIST_LIMIT, IdempotencyReservation,
    PlatformStorage, ReserveResourceCreate, ResourceCreateReservation, ResourceRepository,
    WorkerRepository, decode_catalog_cursor, normalize_catalog_limit,
};
use open_compute_workers::{
    CreateResourceOutcome, CreateResourceRequest, CreateResourceResult, D1ResourceDriver,
    ResourceController, ResourcePins,
};
use serde::{Deserialize, Serialize};
use std::str::FromStr;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const MAX_JSON_BODY: usize = 1024 * 1024;
const IDEMPOTENCY_HEADER: &str = "idempotency-key";
const IDEMPOTENCY_TTL_MS: i64 = 24 * 60 * 60 * 1000;

#[path = "d1_http_backup.rs"]
pub(crate) mod backup;

/// Shared D1 control-plane composition state.
#[derive(Clone)]
pub struct D1ApiState {
    storage: Arc<PlatformStorage>,
    artifacts: ArtifactStore,
    pins: ResourcePins,
    backend: Arc<D1BindingService>,
    config: D1Config,
    max_resources_per_account: u32,
    delete_drain_timeout: Duration,
}

impl std::fmt::Debug for D1ApiState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("D1ApiState")
            .field("artifacts", &self.artifacts)
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl D1ApiState {
    /// Bind central authority, system backup storage, operation lanes, and limits.
    #[must_use]
    pub fn new(
        storage: Arc<PlatformStorage>,
        artifacts: ArtifactStore,
        pins: ResourcePins,
        backend: Arc<D1BindingService>,
        config: D1Config,
        max_resources_per_account: u32,
        delete_drain_timeout: Duration,
    ) -> Self {
        Self {
            storage,
            artifacts,
            pins,
            backend,
            config,
            max_resources_per_account,
            delete_drain_timeout,
        }
    }

    pub(crate) fn storage(&self) -> &Arc<PlatformStorage> {
        &self.storage
    }

    pub(crate) fn pins(&self) -> &ResourcePins {
        &self.pins
    }

    pub(crate) fn backend(&self) -> &Arc<D1BindingService> {
        &self.backend
    }

    pub(crate) const fn config(&self) -> &D1Config {
        &self.config
    }

    pub(crate) const fn delete_drain_timeout(&self) -> Duration {
        self.delete_drain_timeout
    }
}

/// Router for the D1 management surface.
pub fn control_router() -> Router<HttpState> {
    Router::new()
        .route(
            "/v1/accounts/{account_id}/d1/databases",
            post(create_database).get(list_databases),
        )
        .route(
            "/v1/accounts/{account_id}/d1/databases/{resource_id}/migrations",
            get(list_migrations),
        )
        .route(
            "/v1/accounts/{account_id}/d1/databases/{resource_id}/migrations/apply",
            post(apply_migrations),
        )
        .route(
            "/v1/accounts/{account_id}/d1/databases/{resource_id}",
            get(get_database)
                .patch(rename_database)
                .delete(delete_database),
        )
        .route(
            "/v1/accounts/{account_id}/d1/databases/{resource_id}/tables",
            get(list_tables),
        )
        .route(
            "/v1/accounts/{account_id}/d1/databases/{resource_id}/query",
            post(run_query),
        )
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct QueryDatabaseBody {
    sql: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateDatabaseBody {
    name: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ListDatabasesQuery {
    search: Option<String>,
    status: Option<ResourceState>,
    sort: Option<CatalogSort>,
    direction: Option<CatalogDirection>,
    cursor: Option<String>,
    limit: Option<u16>,
}

async fn create_database(
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
    let body = match read_json::<CreateDatabaseBody>(request).await {
        Ok(value) => value,
        Err(error) => return error_response(error, request_id),
    };
    let storage = api.storage.clone();
    let pins = api.pins.clone();
    let quota = api.config.database_quota_bytes;
    match tokio::task::spawn_blocking(move || {
        let driver = D1ResourceDriver::new(&storage, quota);
        ResourceController::new(&storage, pins, driver).create(&CreateResourceRequest {
            account_id,
            kind: BindingKind::D1Database,
            name: body.name,
            idempotency_key: key,
            driver_schema_version: D1_DATABASE_SCHEMA_VERSION,
            request_id,
            now_ms: now_ms(),
        })
    })
    .await
    {
        Ok(Ok(CreateResourceOutcome::Applied(value))) => json_response(&value, StatusCode::CREATED),
        Ok(Ok(CreateResourceOutcome::Replay(bytes))) => json_bytes(bytes, StatusCode::OK),
        Ok(Err(error)) => error_response(error, request_id),
        Err(_) => error_response(internal(), request_id),
    }
}

async fn list_databases(
    State(state): State<HttpState>,
    Path(account): Path<String>,
    query: Result<Query<ListDatabasesQuery>, axum::extract::rejection::QueryRejection>,
    request: Request,
) -> Response {
    let request_id = request_id(&request);
    let Some(api) = authorized_api(&state, &request) else {
        return unauthorized_or_unavailable(&state, &request, request_id);
    };
    let Ok(Query(query)) = query else {
        return error_response(
            PlatformError::new(
                ErrorCode::ConfigInvalid,
                "D1 database list query is invalid",
            ),
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
        D1DatabaseRepository::new(storage.db()).list_page(
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
                "databases": page.items,
                "cursor": page.next_cursor,
                "listComplete": page.next_cursor.is_none(),
            }),
            StatusCode::OK,
        ),
        Ok(Err(error)) => error_response(error, request_id),
        Err(_) => error_response(internal(), request_id),
    }
}

async fn get_database(
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
    let storage = api.storage.clone();
    match tokio::task::spawn_blocking(move || {
        D1DatabaseRepository::new(storage.db()).get(account_id, resource_id)
    })
    .await
    {
        Ok(Ok(database)) => {
            json_response(&serde_json::json!({ "database": database }), StatusCode::OK)
        }
        Ok(Err(error)) => error_response(error, request_id),
        Err(_) => error_response(internal(), request_id),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RenameDatabaseBody {
    name: String,
}

async fn rename_database(
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
    let body = match read_json::<RenameDatabaseBody>(request).await {
        Ok(value) => value,
        Err(error) => return error_response(error, request_id),
    };
    let admission = match api.storage.reserve_mutation(64 * 1024) {
        Ok(value) => value,
        Err(error) => return error_response(error, request_id),
    };
    let storage = api.storage.clone();
    let pins = api.pins.clone();
    let quota = api.config.database_quota_bytes;
    match tokio::task::spawn_blocking(move || {
        let _admission = admission;
        let driver = D1ResourceDriver::new(&storage, quota);
        ResourceController::new(&storage, pins, driver).rename(
            account_id,
            resource_id,
            &body.name,
            request_id,
            now_ms(),
        )
    })
    .await
    {
        Ok(Ok(resource)) => {
            json_response(&serde_json::json!({ "database": resource }), StatusCode::OK)
        }
        Ok(Err(error)) => error_response(error, request_id),
        Err(_) => error_response(internal(), request_id),
    }
}

async fn delete_database(
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
    if let Err(error) = idempotency_key(&request) {
        return error_response(error, request_id);
    }
    let storage = api.storage.clone();
    let current = tokio::task::spawn_blocking(move || {
        ResourceRepository::new(storage.db()).get(account_id, resource_id)
    })
    .await;
    match current {
        Ok(Ok(resource))
            if resource.kind == BindingKind::D1Database
                && resource.state == ResourceState::Tombstoned =>
        {
            return json_response(
                &serde_json::json!({ "resourceId": resource_id, "state": "tombstoned" }),
                StatusCode::OK,
            );
        }
        Ok(Ok(resource)) if resource.kind == BindingKind::D1Database => {}
        Ok(Ok(_)) => return error_response(not_found(), request_id),
        Ok(Err(error)) => return error_response(error, request_id),
        Err(_) => return error_response(internal(), request_id),
    }
    let driver = D1ResourceDriver::new(&api.storage, api.config.database_quota_bytes);
    let controller = ResourceController::new(&api.storage, api.pins.clone(), driver);
    match controller
        .delete(
            account_id,
            resource_id,
            request_id,
            now_ms(),
            api.delete_drain_timeout,
        )
        .await
    {
        Ok(()) => json_response(
            &serde_json::json!({ "resourceId": resource_id, "state": "tombstoned" }),
            StatusCode::ACCEPTED,
        ),
        Err(error) => error_response(error, request_id),
    }
}

async fn list_migrations(
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
    match api.backend.migrations(account_id, resource_id).await {
        Ok(migrations) => json_response(
            &serde_json::json!({ "migrations": migrations }),
            StatusCode::OK,
        ),
        Err(error) => error_response(error, request_id),
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ApplyMigrationsBody {
    migrations: Vec<MigrationBody>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct MigrationBody {
    id: u32,
    name: String,
    sha256: String,
    sql: String,
}

async fn apply_migrations(
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
    let key = match idempotency_key(&request) {
        Ok(value) => value,
        Err(error) => return error_response(error, request_id),
    };
    let body = match read_json::<ApplyMigrationsBody>(request).await {
        Ok(value) => value,
        Err(error) => return error_response(error, request_id),
    };
    let migration_metric = D1LifecycleGuard::new(state.metrics().clone(), D1Lifecycle::Migration);
    let Ok(canonical) = serde_json::to_vec(&body) else {
        return error_response(internal(), request_id);
    };
    let fingerprint = api.storage.crypto().fingerprint_request(&canonical);
    let scope = format!("d1-migrations:{resource_id}");
    let storage = api.storage.clone();
    let scope_for_reservation = scope.clone();
    let key_for_reservation = key.clone();
    let reservation = tokio::task::spawn_blocking(move || {
        D1DatabaseRepository::new(storage.db()).get(account_id, resource_id)?;
        WorkerRepository::new(storage.db()).reserve_idempotency(
            account_id,
            &scope_for_reservation,
            &key_for_reservation,
            storage.crypto().fingerprint_key_id(),
            &fingerprint,
            now_ms(),
            now_ms().saturating_add(IDEMPOTENCY_TTL_MS),
        )
    })
    .await;
    match reservation {
        Ok(Ok(IdempotencyReservation::Complete(bytes))) => {
            migration_metric.success();
            return json_bytes(bytes, StatusCode::OK);
        }
        Ok(Ok(IdempotencyReservation::Failed(bytes))) => {
            return json_bytes(bytes, StatusCode::CONFLICT);
        }
        Ok(Ok(IdempotencyReservation::Running)) => {
            return error_response(
                PlatformError::new(ErrorCode::D1Overloaded, "D1 migration is already running"),
                request_id,
            );
        }
        Ok(Ok(IdempotencyReservation::Reserved)) => {}
        Ok(Err(error)) => return error_response(error, request_id),
        Err(_) => return error_response(internal(), request_id),
    }
    let mut migrations = Vec::with_capacity(body.migrations.len());
    for value in body.migrations {
        let digest: [u8; 32] = match hex::decode(&value.sha256)
            .ok()
            .and_then(|bytes| bytes.try_into().ok())
        {
            Some(value) => value,
            None => {
                let error = PlatformError::new(
                    ErrorCode::D1MigrationDrift,
                    "D1 migration digest is invalid",
                );
                fail_idempotency(
                    api,
                    account_id,
                    &scope,
                    &key,
                    &fingerprint,
                    &error,
                    request_id,
                )
                .await;
                return error_response(error, request_id);
            }
        };
        migrations.push(D1Migration {
            id: value.id,
            name: value.name,
            sha256: digest,
            sql: value.sql,
        });
    }
    match api
        .backend
        .apply_migrations(account_id, resource_id, migrations, now_ms())
        .await
    {
        Ok(migrations) => {
            let response = serde_json::to_vec(&serde_json::json!({ "migrations": migrations }))
                .unwrap_or_default();
            let storage = api.storage.clone();
            let scope = scope.clone();
            let key = key.clone();
            let completed = tokio::task::spawn_blocking(move || {
                WorkerRepository::new(storage.db()).complete_idempotency(
                    account_id,
                    &scope,
                    &key,
                    &fingerprint,
                    &response,
                )?;
                Ok::<_, PlatformError>(response)
            })
            .await;
            match completed {
                Ok(Ok(response)) => {
                    migration_metric.success();
                    json_bytes(response, StatusCode::OK)
                }
                Ok(Err(error)) => error_response(error, request_id),
                Err(_) => error_response(internal(), request_id),
            }
        }
        Err(error) => {
            fail_idempotency(
                api,
                account_id,
                &scope,
                &key,
                &fingerprint,
                &error,
                request_id,
            )
            .await;
            error_response(error, request_id)
        }
    }
}

fn authorized_api<'a>(state: &'a HttpState, request: &Request) -> Option<&'a Arc<D1ApiState>> {
    if authorize(state, request) {
        state.d1_api()
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
        [(axum::http::header::CONTENT_TYPE, "application/json")],
        Body::from(bytes),
    )
        .into_response()
}

#[allow(clippy::needless_pass_by_value)]
fn error_response(error: PlatformError, request_id: RequestId) -> Response {
    let code = error.code();
    let status = match code {
        ErrorCode::ResourceNotFound => StatusCode::NOT_FOUND,
        ErrorCode::AdminAuthRequired => StatusCode::UNAUTHORIZED,
        ErrorCode::BindingPermissionDenied => StatusCode::FORBIDDEN,
        ErrorCode::ResourceNameConflict
        | ErrorCode::IdempotencyConflict
        | ErrorCode::ResourceReferenced
        | ErrorCode::ResourceNotReady
        | ErrorCode::D1MigrationDrift => StatusCode::CONFLICT,
        ErrorCode::ConfigInvalid
        | ErrorCode::LimitInvalid
        | ErrorCode::D1TypeError
        | ErrorCode::D1SqlInvalid
        | ErrorCode::D1ParameterMismatch
        | ErrorCode::D1AuthorizerDenied
        | ErrorCode::D1InvalidBatch => StatusCode::BAD_REQUEST,
        ErrorCode::D1LimitError => StatusCode::PAYLOAD_TOO_LARGE,
        ErrorCode::D1DatabaseCorrupt
        | ErrorCode::D1IdentityMismatch
        | ErrorCode::ArtifactIntegrityError => StatusCode::UNPROCESSABLE_ENTITY,
        ErrorCode::D1Overloaded | ErrorCode::QuotaExceeded | ErrorCode::AdmissionBusy => {
            StatusCode::TOO_MANY_REQUESTS
        }
        ErrorCode::StoragePressure | ErrorCode::DiskHardLimit | ErrorCode::D1DatabaseFull => {
            StatusCode::INSUFFICIENT_STORAGE
        }
        ErrorCode::PlatformUnavailable => StatusCode::SERVICE_UNAVAILABLE,
        ErrorCode::D1Timeout
        | ErrorCode::D1ResultUnknown
        | ErrorCode::S3Unavailable
        | ErrorCode::ArtifactUnavailable => StatusCode::SERVICE_UNAVAILABLE,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    };
    let mut response = (
        status,
        axum::Json(serde_json::json!({
            "ok": false,
            "error": {
                "code": code.as_str(),
                "message": "control request failed",
                "requestId": request_id,
            }
        })),
    )
        .into_response();
    response.extensions_mut().insert(ProductErrorCode(code));
    response
}

async fn fail_idempotency(
    api: &D1ApiState,
    account_id: AccountId,
    scope: &str,
    key: &str,
    fingerprint: &[u8; 32],
    error: &PlatformError,
    request_id: RequestId,
) {
    let storage = api.storage.clone();
    let scope = scope.to_owned();
    let key = key.to_owned();
    let fingerprint = *fingerprint;
    let body = serde_json::to_vec(&serde_json::json!({
        "ok": false,
        "error": { "code": error.code().as_str(), "message": "control request failed", "requestId": request_id }
    }))
    .unwrap_or_default();
    let _ = tokio::task::spawn_blocking(move || {
        WorkerRepository::new(storage.db()).fail_idempotency(
            account_id,
            &scope,
            &key,
            &fingerprint,
            &body,
        )
    })
    .await;
}

fn not_found() -> PlatformError {
    PlatformError::new(ErrorCode::ResourceNotFound, "D1 database was not found")
}

async fn list_tables(
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
    match api
        .backend
        .operator_list_tables(account_id, resource_id)
        .await
    {
        Ok(tables) => json_response(
            &serde_json::json!({
                "tables": tables.into_iter().map(|name| serde_json::json!({ "name": name })).collect::<Vec<_>>(),
            }),
            StatusCode::OK,
        ),
        Err(error) => error_response(error, request_id),
    }
}

async fn run_query(
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
    let body = match read_json::<QueryDatabaseBody>(request).await {
        Ok(value) => value,
        Err(error) => return error_response(error, request_id),
    };
    match api
        .backend
        .operator_query(account_id, resource_id, body.sql)
        .await
    {
        Ok(result) => json_response(&d1_query_response(&result), StatusCode::OK),
        Err(error) => error_response(error, request_id),
    }
}

fn d1_query_response(result: &open_compute_storage::D1StatementResult) -> serde_json::Value {
    let results = result
        .rows
        .iter()
        .map(|row| {
            let mut record = serde_json::Map::new();
            for (column, value) in result.columns.iter().zip(row.iter()) {
                record.insert(column.clone(), d1_value_json(value));
            }
            serde_json::Value::Object(record)
        })
        .collect::<Vec<_>>();
    serde_json::json!({
        "results": results,
        "meta": {
            "durationMs": result.meta.duration,
            "rowsRead": result.meta.rows_read,
            "rowsWritten": result.meta.rows_written,
        },
    })
}

fn d1_value_json(value: &open_compute_storage::D1Value) -> serde_json::Value {
    use open_compute_storage::D1Value;
    match value {
        D1Value::Null => serde_json::Value::Null,
        D1Value::Integer(value) => serde_json::json!(value),
        D1Value::Real(value) => serde_json::json!(value),
        D1Value::Text(value) => serde_json::json!(value),
        D1Value::Blob(value) => serde_json::json!(STANDARD.encode(value)),
    }
}

fn internal() -> PlatformError {
    PlatformError::new(ErrorCode::Internal, "D1 control operation failed")
}

#[cfg(test)]
#[path = "d1_http_tests.rs"]
mod tests;
