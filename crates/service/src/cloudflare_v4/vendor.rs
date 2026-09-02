//! open-compute vendor extensions backed by existing domain authorities.

use super::{
    V4Error, V4Permission, V4RequestContext, V4ResourceKind, error_response, request_context,
    success_response,
};
use crate::http::HttpState;
use axum::Router;
use axum::body::to_bytes;
use axum::extract::{Path, Request, State};
use axum::response::Response;
use axum::routing::{get, post};
use open_compute_core::{AccountId, BindingKind, ErrorCode, PlatformError, ResourceId};
use open_compute_storage::{
    D1BackupRecord, D1DatabaseRepository, DurableObjectRepository, KvBackupRecord,
    KvNamespaceRepository, ResourceRepository, WorkerOwnership, WorkerRepository,
};
use serde::Serialize;
use std::collections::BTreeMap;

const WRANGLER_VERSION: &str = "4.127.1";

pub(super) fn router() -> Router<HttpState> {
    Router::new()
        .route("/open-compute/capabilities", get(capabilities))
        .route("/open-compute/system/status", get(system_status))
        .route("/open-compute/scheduler", get(scheduler_status))
        .route("/open-compute/scheduler/pause", post(scheduler_pause))
        .route("/open-compute/scheduler/resume", post(scheduler_resume))
        .route("/open-compute/scheduler/repair", post(scheduler_repair))
        .route("/open-compute/cache", get(cache_status))
        .route(
            "/open-compute/cache/garbage-collection",
            post(cache_garbage_collection),
        )
        .route("/open-compute/images/capacity", get(image_capacity))
        .route(
            "/accounts/{account_id}/open-compute/workers/{script_name}/endpoints",
            get(worker_endpoints),
        )
        .route(
            "/accounts/{account_id}/open-compute/durable-objects",
            get(durable_object_namespaces),
        )
        .route(
            "/accounts/{account_id}/open-compute/durable-objects/{namespace_id}/objects",
            get(durable_object_records),
        )
        .route(
            "/accounts/{account_id}/open-compute/kv/namespaces/{namespace_id}/backups",
            get(kv_backups).post(unsupported_maintenance),
        )
        .route(
            "/accounts/{account_id}/open-compute/kv/backups/{backup_id}/restore",
            post(unsupported_maintenance),
        )
        .route(
            "/accounts/{account_id}/open-compute/d1/databases/{database_id}/backups",
            get(d1_backups).post(unsupported_maintenance),
        )
        .route(
            "/accounts/{account_id}/open-compute/d1/backups/{backup_id}/restore",
            post(unsupported_maintenance),
        )
}

async fn capabilities(State(_state): State<HttpState>, request: Request) -> Response {
    let context = match read_context(&request, V4Permission::Read) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let lock = match open_compute_runtime::embedded_runtime_lock() {
        Ok((value, _)) => value,
        Err(error) => return platform_error(error, context),
    };
    let inventory: serde_json::Value = match serde_json::from_slice(include_bytes!(
        "../../../../share/cloudflare-capabilities.json"
    )) {
        Ok(value) => value,
        Err(_) => return error_response(V4Error::Internal, context.request_id()),
    };
    let mut endpoints = BTreeMap::new();
    if let Some(routes) = inventory
        .pointer("/managementApi/routes")
        .and_then(serde_json::Value::as_array)
    {
        for route in routes {
            let operation_id = route
                .get("operationId")
                .and_then(serde_json::Value::as_str)
                .filter(|value| !value.is_empty())
                .or_else(|| route.get("id").and_then(serde_json::Value::as_str));
            let status = route.get("status").and_then(serde_json::Value::as_str);
            let (Some(operation_id), Some(status)) = (operation_id, status) else {
                return error_response(V4Error::Internal, context.request_id());
            };
            endpoints.insert(
                operation_id,
                match status {
                    "supported" => "supported",
                    "supported_with_deviation" => "supported_with_deviation",
                    _ => "unsupported",
                },
            );
        }
    }
    success_response(
        context,
        Capabilities {
            release: env!("CARGO_PKG_VERSION"),
            wrangler_version: WRANGLER_VERSION,
            compatibility_date: CompatibilityDate {
                minimum: &lock.effective_compatibility_date,
                maximum: &lock.effective_compatibility_date,
            },
            compatibility_flags: &lock.required_compatibility_flags,
            endpoints,
            deviations: ["OC-MANAGEMENT-COMPATIBILITY-DATE-001"],
        },
    )
}

async fn system_status(State(state): State<HttpState>, request: Request) -> Response {
    let context = match read_context(&request, V4Permission::Read) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let snapshot = state.health().snapshot();
    let components = snapshot
        .components
        .into_iter()
        .map(|component| StatusComponent {
            name: component.name.as_str(),
            state: component.state.as_str(),
            message: component.reason.map(|reason| reason.as_str()),
        })
        .collect();
    success_response(
        context,
        SystemStatus {
            state: snapshot.readiness.as_str(),
            version: env!("CARGO_PKG_VERSION"),
            components,
        },
    )
}

async fn scheduler_status(State(state): State<HttpState>, request: Request) -> Response {
    let context = match read_context(&request, V4Permission::Read) {
        Ok(value) => value,
        Err(response) => return response,
    };
    scheduler_response(&state, context)
}

async fn scheduler_pause(State(state): State<HttpState>, request: Request) -> Response {
    let context = match bodyless_context(request, V4Permission::Maintenance).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let Some(scheduler) = state.scheduler() else {
        return error_response(V4Error::Unavailable, context.request_id());
    };
    scheduler.pause();
    scheduler_response(&state, context)
}

async fn scheduler_resume(State(state): State<HttpState>, request: Request) -> Response {
    let context = match bodyless_context(request, V4Permission::Maintenance).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let Some(scheduler) = state.scheduler() else {
        return error_response(V4Error::Unavailable, context.request_id());
    };
    scheduler.resume();
    scheduler_response(&state, context)
}

async fn scheduler_repair(State(state): State<HttpState>, request: Request) -> Response {
    let context = match bodyless_context(request, V4Permission::Maintenance).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let Some(scheduler) = state.scheduler() else {
        return error_response(V4Error::Unavailable, context.request_id());
    };
    if let Err(error) = scheduler.repair_and_probe().await {
        return platform_error(error, context);
    }
    if let Err(error) = scheduler.repair_products(1_000) {
        return platform_error(error, context);
    }
    scheduler_response(&state, context)
}

fn scheduler_response(state: &HttpState, context: V4RequestContext) -> Response {
    let Some(scheduler) = state.scheduler() else {
        return error_response(V4Error::Unavailable, context.request_id());
    };
    match scheduler.inspect() {
        Ok(value) => success_response(
            context,
            SchedulerStatus {
                state: if value.paused { "paused" } else { "running" },
                pending: value
                    .pools
                    .iter()
                    .map(|pool| pool.ready.saturating_add(pool.expired))
                    .sum(),
                running: value.global.in_flight,
            },
        ),
        Err(error) => platform_error(error, context),
    }
}

async fn cache_status(State(state): State<HttpState>, request: Request) -> Response {
    let context = match read_context(&request, V4Permission::Read) {
        Ok(value) => value,
        Err(response) => return response,
    };
    cache_response(&state, context)
}

async fn cache_garbage_collection(State(state): State<HttpState>, request: Request) -> Response {
    let context = match bodyless_context(request, V4Permission::Maintenance).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let Some(api) = state.cache_images_api() else {
        return error_response(V4Error::Unavailable, context.request_id());
    };
    if let Err(error) = api.garbage_collect().await {
        return platform_error(error, context);
    }
    cache_response(&state, context)
}

fn cache_response(state: &HttpState, context: V4RequestContext) -> Response {
    let Some(api) = state.cache_images_api() else {
        return error_response(V4Error::Unavailable, context.request_id());
    };
    match api.cache_stats() {
        Ok(stats) => success_response(
            context,
            CacheStatus {
                entries: stats.entries,
                bytes: stats.body_bytes.saturating_add(stats.metadata_bytes),
            },
        ),
        Err(error) => platform_error(error, context),
    }
}

async fn image_capacity(State(state): State<HttpState>, request: Request) -> Response {
    let context = match read_context(&request, V4Permission::Read) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let Some(api) = state.cache_images_api() else {
        return error_response(V4Error::Unavailable, context.request_id());
    };
    match api.image_capacity() {
        Ok(value) => success_response(
            context,
            ImageCapacity {
                queued: value.active_sessions,
                running: value.active_transforms,
                capacity: value.max_concurrency,
            },
        ),
        Err(error) => platform_error(error, context),
    }
}

async fn worker_endpoints(
    State(state): State<HttpState>,
    Path((account, script_name)): Path<(String, String)>,
    request: Request,
) -> Response {
    let context = match read_context(&request, V4Permission::Read) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let account = match resolve_account(&state, &account) {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    let Some(storage) = state.platform_storage() else {
        return error_response(V4Error::Unavailable, context.request_id());
    };
    let workers = WorkerRepository::new(storage.db());
    let worker = match workers.list_workers(account).and_then(|workers| {
        workers
            .into_iter()
            .find(|worker| {
                worker.name == script_name && worker.ownership == WorkerOwnership::Tenant
            })
            .ok_or_else(|| PlatformError::new(ErrorCode::WorkerNotFound, "Worker not found"))
    }) {
        Ok(value) => value,
        Err(error) => return platform_error(error, context),
    };
    match workers.list_routes(account, worker.id) {
        Ok(routes) => {
            let result = routes
                .into_iter()
                .map(|route| {
                    Ok(WorkerEndpoint {
                        id: route.id,
                        path: route.path_prefix,
                        created_on: timestamp(worker.created_at_ms)?,
                    })
                })
                .collect::<Result<Vec<_>, V4Error>>();
            match result {
                Ok(result) => success_response(context, result),
                Err(error) => error_response(error, context.request_id()),
            }
        }
        Err(error) => platform_error(error, context),
    }
}

async fn durable_object_namespaces(
    State(state): State<HttpState>,
    Path(account): Path<String>,
    request: Request,
) -> Response {
    let context = match read_context(&request, V4Permission::Read) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let account = match resolve_account(&state, &account) {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    let Some(storage) = state.platform_storage() else {
        return error_response(V4Error::Unavailable, context.request_id());
    };
    let Some(authority) = state.cloudflare_v4_account() else {
        return error_response(V4Error::Unavailable, context.request_id());
    };
    let workers = WorkerRepository::new(storage.db());
    match DurableObjectRepository::new(storage).list_namespaces(account) {
        Ok(namespaces) => {
            let result = namespaces
                .into_iter()
                .map(|namespace| {
                    workers
                        .get_worker(account, namespace.owner_worker_id)
                        .map(|worker| DurableObjectNamespace {
                            id: authority.public_resource_id(
                                V4ResourceKind::DurableObjectNamespace,
                                namespace.resource.id,
                            ),
                            script_name: worker.name,
                            class_name: namespace.class_name,
                        })
                })
                .collect::<Result<Vec<_>, _>>();
            match result {
                Ok(result) => success_response(context, result),
                Err(error) => platform_error(error, context),
            }
        }
        Err(error) => platform_error(error, context),
    }
}

async fn durable_object_records(
    State(state): State<HttpState>,
    Path((account, namespace_public)): Path<(String, String)>,
    request: Request,
) -> Response {
    let context = match read_context(&request, V4Permission::Read) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let (account, namespace) = match resolve_resource(
        &state,
        &account,
        &namespace_public,
        V4ResourceKind::DurableObjectNamespace,
        BindingKind::DoNamespace,
    ) {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    let Some(storage) = state.platform_storage() else {
        return error_response(V4Error::Unavailable, context.request_id());
    };
    match DurableObjectRepository::new(storage).list_objects(account, namespace) {
        Ok(objects) => {
            let result = objects
                .into_iter()
                .map(|object| {
                    Ok(DurableObjectRecord {
                        id: object.object_id.to_string(),
                        namespace_id: namespace_public.clone(),
                        created_on: timestamp(object.created_at_ms)?,
                    })
                })
                .collect::<Result<Vec<_>, V4Error>>();
            match result {
                Ok(result) => success_response(context, result),
                Err(error) => error_response(error, context.request_id()),
            }
        }
        Err(error) => platform_error(error, context),
    }
}

async fn kv_backups(
    State(state): State<HttpState>,
    Path((account, namespace)): Path<(String, String)>,
    request: Request,
) -> Response {
    let context = match read_context(&request, V4Permission::Read) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let (account, namespace) = match resolve_resource(
        &state,
        &account,
        &namespace,
        V4ResourceKind::KvNamespace,
        BindingKind::KvNamespace,
    ) {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    let Some(storage) = state.platform_storage() else {
        return error_response(V4Error::Unavailable, context.request_id());
    };
    match KvNamespaceRepository::new(storage.db()).list_backups(account) {
        Ok(backups) => backup_list_response(
            context,
            backups
                .into_iter()
                .filter(|backup| backup.source_resource_id == namespace)
                .map(Backup::try_from)
                .collect(),
        ),
        Err(error) => platform_error(error, context),
    }
}

async fn d1_backups(
    State(state): State<HttpState>,
    Path((account, database)): Path<(String, String)>,
    request: Request,
) -> Response {
    let context = match read_context(&request, V4Permission::Read) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let (account, database) = match resolve_resource(
        &state,
        &account,
        &database,
        V4ResourceKind::D1Database,
        BindingKind::D1Database,
    ) {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    let Some(storage) = state.platform_storage() else {
        return error_response(V4Error::Unavailable, context.request_id());
    };
    match D1DatabaseRepository::new(storage.db()).list_backups(account, database) {
        Ok(backups) => {
            backup_list_response(context, backups.into_iter().map(Backup::try_from).collect())
        }
        Err(error) => platform_error(error, context),
    }
}

async fn unsupported_maintenance(State(_state): State<HttpState>, request: Request) -> Response {
    let context = match bodyless_context(request, V4Permission::Maintenance).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    error_response(V4Error::Unsupported, context.request_id())
}

fn backup_list_response(
    context: V4RequestContext,
    backups: Result<Vec<Backup>, V4Error>,
) -> Response {
    match backups {
        Ok(value) => success_response(context, value),
        Err(error) => error_response(error, context.request_id()),
    }
}

fn resolve_account(state: &HttpState, public: &str) -> Result<AccountId, V4Error> {
    state
        .cloudflare_v4_account()
        .ok_or(V4Error::Unavailable)?
        .resolve(public)
}

fn resolve_resource(
    state: &HttpState,
    public_account: &str,
    public_resource: &str,
    kind: V4ResourceKind,
    binding_kind: BindingKind,
) -> Result<(AccountId, ResourceId), V4Error> {
    let account = resolve_account(state, public_account)?;
    let authority = state.cloudflare_v4_account().ok_or(V4Error::Unavailable)?;
    let storage = state.platform_storage().ok_or(V4Error::Unavailable)?;
    let resource = ResourceRepository::new(storage.db())
        .list(account, Some(binding_kind))
        .map_err(|error| V4Error::from(&error))?
        .into_iter()
        .find(|resource| authority.matches_public_resource_id(kind, resource.id, public_resource))
        .map(|resource| resource.id)
        .ok_or(V4Error::NotFound)?;
    Ok((account, resource))
}

fn read_context(request: &Request, permission: V4Permission) -> Result<V4RequestContext, Response> {
    let context = request_context(request)?;
    if request.uri().query().is_some() {
        return Err(error_response(
            V4Error::InvalidRequest,
            context.request_id(),
        ));
    }
    context
        .require(permission)
        .map_err(|error| error_response(error, context.request_id()))?;
    Ok(context)
}

async fn bodyless_context(
    request: Request,
    permission: V4Permission,
) -> Result<V4RequestContext, Response> {
    let context = read_context(&request, permission)?;
    let bytes = to_bytes(request.into_body(), 1)
        .await
        .map_err(|_| error_response(V4Error::InvalidRequest, context.request_id()))?;
    if !bytes.is_empty() {
        return Err(error_response(
            V4Error::InvalidRequest,
            context.request_id(),
        ));
    }
    Ok(context)
}

fn platform_error(error: PlatformError, context: V4RequestContext) -> Response {
    error_response(V4Error::from(&error), context.request_id())
}

fn timestamp(value: i64) -> Result<String, V4Error> {
    jiff::Timestamp::from_millisecond(value)
        .map(|timestamp| timestamp.to_string())
        .map_err(|_| V4Error::Internal)
}

#[derive(Serialize)]
struct Capabilities<'a> {
    release: &'a str,
    wrangler_version: &'a str,
    compatibility_date: CompatibilityDate<'a>,
    compatibility_flags: &'a [String],
    endpoints: BTreeMap<&'a str, &'static str>,
    deviations: [&'static str; 1],
}

#[derive(Serialize)]
struct CompatibilityDate<'a> {
    minimum: &'a str,
    maximum: &'a str,
}

#[derive(Serialize)]
struct SystemStatus {
    state: &'static str,
    version: &'static str,
    components: Vec<StatusComponent>,
}

#[derive(Serialize)]
struct StatusComponent {
    name: &'static str,
    state: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<&'static str>,
}

#[derive(Serialize)]
struct SchedulerStatus {
    state: &'static str,
    pending: u64,
    running: usize,
}

#[derive(Serialize)]
struct CacheStatus {
    entries: u64,
    bytes: u64,
}

#[derive(Serialize)]
struct ImageCapacity {
    queued: u64,
    running: u64,
    capacity: u16,
}

#[derive(Serialize)]
struct WorkerEndpoint {
    id: String,
    path: String,
    created_on: String,
}

#[derive(Serialize)]
struct DurableObjectNamespace {
    id: String,
    script_name: String,
    class_name: String,
}

#[derive(Serialize)]
struct DurableObjectRecord {
    id: String,
    namespace_id: String,
    created_on: String,
}

#[derive(Serialize)]
struct Backup {
    id: String,
    created_on: String,
    state: &'static str,
    size: u64,
}

impl TryFrom<KvBackupRecord> for Backup {
    type Error = V4Error;

    fn try_from(value: KvBackupRecord) -> Result<Self, Self::Error> {
        Ok(Self {
            id: value.id,
            created_on: timestamp(value.created_at_ms)?,
            state: value.state.as_str(),
            size: value.size_bytes.unwrap_or(0),
        })
    }
}

impl TryFrom<D1BackupRecord> for Backup {
    type Error = V4Error;

    fn try_from(value: D1BackupRecord) -> Result<Self, Self::Error> {
        Ok(Self {
            id: value.id,
            created_on: timestamp(value.created_at_ms)?,
            state: value.state.as_str(),
            size: value.size_bytes.unwrap_or(0),
        })
    }
}
