//! Vendor backup routes backed by the durable KV and D1 workflow authorities.

use super::{
    error_response, read_context, resolve_account, resolve_resource, success_response, timestamp,
};
use crate::cloudflare_v4::{V4Error, V4Permission, V4RequestContext, V4ResourceKind};
use crate::http::HttpState;
use crate::metrics::{D1Lifecycle, D1LifecycleGuard, KvLifecycle, KvLifecycleGuard};
use axum::Router;
use axum::body::to_bytes;
use axum::extract::{Path, Request, State};
use axum::http::header::CONTENT_TYPE;
use axum::response::Response;
use axum::routing::{get, post};
use open_compute_core::{BindingKind, ErrorCode, PlatformError, ResourceId};
use open_compute_storage::{
    D1BackupRecord, D1DatabaseRepository, KvBackupRecord, KvNamespaceRepository, ResourceRepository,
};
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_RESTORE_BODY: usize = 4096;
const IDEMPOTENCY_HEADER: &str = "idempotency-key";

pub(super) fn router() -> Router<HttpState> {
    Router::new()
        .route(
            "/accounts/{account_id}/open-compute/kv/namespaces/{namespace_id}/backups",
            get(kv_backups).post(create_kv_backup),
        )
        .route(
            "/accounts/{account_id}/open-compute/kv/backups/{backup_id}/restore",
            post(restore_kv_backup),
        )
        .route(
            "/accounts/{account_id}/open-compute/d1/databases/{database_id}/backups",
            get(d1_backups).post(create_d1_backup),
        )
        .route(
            "/accounts/{account_id}/open-compute/d1/backups/{backup_id}/restore",
            post(restore_d1_backup),
        )
}

async fn kv_backups(
    State(state): State<HttpState>,
    Path((account, namespace)): Path<(String, String)>,
    request: Request,
) -> Response {
    let context = match bodyless_request(request, V4Permission::Read).await {
        Ok((value, _)) => value,
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
    let storage = storage.clone();
    match tokio::task::spawn_blocking(move || {
        KvNamespaceRepository::new(storage.db()).list_backups(account)
    })
    .await
    {
        Ok(Ok(backups)) => backup_list_response(
            context,
            backups
                .into_iter()
                .filter(|backup| backup.source_resource_id == namespace)
                .map(Backup::try_from)
                .collect(),
        ),
        Ok(Err(error)) => backup_platform_error(&error, context),
        Err(_) => error_response(V4Error::Internal, context.request_id()),
    }
}

async fn create_kv_backup(
    State(state): State<HttpState>,
    Path((account, namespace)): Path<(String, String)>,
    request: Request,
) -> Response {
    let (context, key) = match bodyless_request(request, V4Permission::Maintenance).await {
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
    let Some(api) = state.kv_api() else {
        return error_response(V4Error::Unavailable, context.request_id());
    };
    let now_ms = match checked_now_ms() {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    let metric = KvLifecycleGuard::new(state.metrics().clone(), KvLifecycle::Backup);
    match crate::kv_api::backup::create_backup(
        api,
        account,
        namespace,
        effective_idempotency_key(key, context),
        now_ms,
    )
    .await
    {
        Ok(record) => match Backup::try_from(record) {
            Ok(result) => {
                metric.success();
                success_response(context, result)
            }
            Err(error) => error_response(error, context.request_id()),
        },
        Err(error) => backup_platform_error(&error, context),
    }
}

async fn restore_kv_backup(
    State(state): State<HttpState>,
    Path((account, backup_id)): Path<(String, String)>,
    request: Request,
) -> Response {
    let (context, key, body) = match restore_request(request).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let account = match resolve_account(&state, &account) {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    let Some(api) = state.kv_api() else {
        return error_response(V4Error::Unavailable, context.request_id());
    };
    let now_ms = match checked_now_ms() {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    let metric = KvLifecycleGuard::new(state.metrics().clone(), KvLifecycle::Restore);
    match crate::kv_api::backup::restore_backup(
        api,
        state.metrics(),
        account,
        backup_id,
        body.name,
        effective_idempotency_key(key, context),
        context.request_id(),
        now_ms,
    )
    .await
    {
        Ok(resource_id) => match restored_resource(
            &state,
            account,
            resource_id,
            V4ResourceKind::KvNamespace,
            "kv_namespace",
        ) {
            Ok(result) => {
                metric.success();
                success_response(context, result)
            }
            Err(error) => error_response(error, context.request_id()),
        },
        Err(error) => backup_platform_error(&error, context),
    }
}

async fn d1_backups(
    State(state): State<HttpState>,
    Path((account, database)): Path<(String, String)>,
    request: Request,
) -> Response {
    let context = match bodyless_request(request, V4Permission::Read).await {
        Ok((value, _)) => value,
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
    let storage = storage.clone();
    match tokio::task::spawn_blocking(move || {
        D1DatabaseRepository::new(storage.db()).list_backups(account, database)
    })
    .await
    {
        Ok(Ok(backups)) => {
            backup_list_response(context, backups.into_iter().map(Backup::try_from).collect())
        }
        Ok(Err(error)) => backup_platform_error(&error, context),
        Err(_) => error_response(V4Error::Internal, context.request_id()),
    }
}

async fn create_d1_backup(
    State(state): State<HttpState>,
    Path((account, database)): Path<(String, String)>,
    request: Request,
) -> Response {
    let (context, key) = match bodyless_request(request, V4Permission::Maintenance).await {
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
    let Some(api) = state.d1_api() else {
        return error_response(V4Error::Unavailable, context.request_id());
    };
    let now_ms = match checked_now_ms() {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    let metric = D1LifecycleGuard::new(state.metrics().clone(), D1Lifecycle::Backup);
    match crate::d1_backup::create_backup(
        api,
        account,
        database,
        effective_idempotency_key(key, context),
        now_ms,
    )
    .await
    {
        Ok(record) => match Backup::try_from(record) {
            Ok(result) => {
                metric.success();
                success_response(context, result)
            }
            Err(error) => error_response(error, context.request_id()),
        },
        Err(error) => backup_platform_error(&error, context),
    }
}

async fn restore_d1_backup(
    State(state): State<HttpState>,
    Path((account, backup_id)): Path<(String, String)>,
    request: Request,
) -> Response {
    let (context, key, body) = match restore_request(request).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let account = match resolve_account(&state, &account) {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    let Some(api) = state.d1_api() else {
        return error_response(V4Error::Unavailable, context.request_id());
    };
    let now_ms = match checked_now_ms() {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    let metric = D1LifecycleGuard::new(state.metrics().clone(), D1Lifecycle::Restore);
    match crate::d1_backup::restore_backup(
        api,
        account,
        backup_id,
        body.name,
        effective_idempotency_key(key, context),
        context.request_id(),
        now_ms,
    )
    .await
    {
        Ok(resource_id) => match restored_resource(
            &state,
            account,
            resource_id,
            V4ResourceKind::D1Database,
            "d1_database",
        ) {
            Ok(result) => {
                metric.success();
                success_response(context, result)
            }
            Err(error) => error_response(error, context.request_id()),
        },
        Err(error) => backup_platform_error(&error, context),
    }
}

fn restored_resource(
    state: &HttpState,
    account: open_compute_core::AccountId,
    resource_id: ResourceId,
    kind: V4ResourceKind,
    public_kind: &'static str,
) -> Result<RestoredResource, V4Error> {
    let storage = state.platform_storage().ok_or(V4Error::Unavailable)?;
    let authority = state.cloudflare_v4_account().ok_or(V4Error::Unavailable)?;
    let record = ResourceRepository::new(storage.db())
        .get(account, resource_id)
        .map_err(|error| V4Error::from(&error))?;
    Ok(RestoredResource {
        id: authority.public_resource_id(kind, record.id),
        name: record.name,
        kind: public_kind,
        created_on: timestamp(record.created_at_ms)?,
    })
}

fn backup_platform_error(error: &PlatformError, context: V4RequestContext) -> Response {
    let wire = match error.code() {
        ErrorCode::ArtifactIntegrityError => V4Error::IntegrityFailure,
        ErrorCode::ArtifactUnavailable | ErrorCode::S3Unavailable => V4Error::Unavailable,
        _ => V4Error::from(error),
    };
    error_response(wire, context.request_id())
}

async fn bodyless_request(
    request: Request,
    permission: V4Permission,
) -> Result<(V4RequestContext, Option<String>), Response> {
    let context = read_context(&request, permission)?;
    let key = optional_idempotency_key(&request)
        .map_err(|error| error_response(error, context.request_id()))?;
    let bytes = to_bytes(request.into_body(), 1)
        .await
        .map_err(|_| error_response(V4Error::InvalidRequest, context.request_id()))?;
    if !bytes.is_empty() {
        return Err(error_response(
            V4Error::InvalidRequest,
            context.request_id(),
        ));
    }
    Ok((context, key))
}

async fn restore_request(
    request: Request,
) -> Result<(V4RequestContext, Option<String>, RestoreRequest), Response> {
    let context = read_context(&request, V4Permission::Maintenance)?;
    let key = optional_idempotency_key(&request)
        .map_err(|error| error_response(error, context.request_id()))?;
    let content_types = request.headers().get_all(CONTENT_TYPE);
    let mut content_types = content_types.iter();
    let valid_content_type = content_types
        .next()
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .is_some_and(|value| value.trim().eq_ignore_ascii_case("application/json"))
        && content_types.next().is_none();
    if !valid_content_type {
        return Err(error_response(
            V4Error::InvalidRequest,
            context.request_id(),
        ));
    }
    let bytes = to_bytes(request.into_body(), MAX_RESTORE_BODY)
        .await
        .map_err(|_| error_response(V4Error::InvalidRequest, context.request_id()))?;
    let body = serde_json::from_slice(&bytes)
        .map_err(|_| error_response(V4Error::InvalidRequest, context.request_id()))?;
    Ok((context, key, body))
}

fn optional_idempotency_key(request: &Request) -> Result<Option<String>, V4Error> {
    let values = request.headers().get_all(IDEMPOTENCY_HEADER);
    let mut values = values.iter();
    let Some(value) = values.next() else {
        return Ok(None);
    };
    if values.next().is_some() {
        return Err(V4Error::InvalidRequest);
    }
    let value = value.to_str().map_err(|_| V4Error::InvalidRequest)?;
    if value.is_empty()
        || value.len() > 128
        || value
            .bytes()
            .any(|byte| byte.is_ascii_control() || byte.is_ascii_whitespace())
    {
        return Err(V4Error::InvalidRequest);
    }
    Ok(Some(value.to_owned()))
}

fn effective_idempotency_key(value: Option<String>, context: V4RequestContext) -> String {
    value.unwrap_or_else(|| format!("v4-{}", context.request_id()))
}

fn checked_now_ms() -> Result<i64, V4Error> {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| V4Error::Internal)?;
    i64::try_from(duration.as_millis()).map_err(|_| V4Error::Internal)
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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RestoreRequest {
    name: String,
}

#[derive(Serialize)]
struct RestoredResource {
    id: String,
    name: String,
    kind: &'static str,
    created_on: String,
}

#[derive(Serialize)]
struct Backup {
    id: String,
    created_on: String,
    state: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    size: Option<u64>,
}

impl TryFrom<KvBackupRecord> for Backup {
    type Error = V4Error;

    fn try_from(value: KvBackupRecord) -> Result<Self, Self::Error> {
        Ok(Self {
            id: value.id,
            created_on: timestamp(value.created_at_ms)?,
            state: value.state.as_str(),
            size: value.size_bytes,
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
            size: value.size_bytes,
        })
    }
}
