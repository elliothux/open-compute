//! Immutable Worker settings, secret, schedule, and deletion adapters.

use super::domain;
use super::handlers::{authorize, json_body, now_ms, platform_error, respond, worker_api};
use crate::cloudflare_v4::{HttpError, V4Error, V4Permission, error_response, success_response};
use crate::http::HttpState;
use axum::extract::{FromRequest, Multipart, Path, Request, State};
use axum::response::Response;
use open_compute_core::{ErrorCode, PlatformError, SecretString};
use open_compute_storage::{
    CronRepository, UpdateWorkerObservabilitySettings, VersionSnapshot, WorkerRecord,
    WorkerRepository,
};
use open_compute_workers::CreateVersionOutcome;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub(super) async fn delete_script(
    State(state): State<HttpState>,
    Path((account, script)): Path<(String, String)>,
    request: Request,
) -> Response {
    let context = match authorize(&request, V4Permission::ProductWrite) {
        Ok(value) => value,
        Err(response) => return response.into_response(),
    };
    match delete_force_query(request.uri().query()) {
        Ok(false) => {}
        Ok(true) => return error_response(V4Error::Unsupported, context.request_id()),
        Err(error) => return error_response(error, context.request_id()),
    }
    let account = match domain::resolve_account(&state, &account) {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    let Some(api) = state.worker_api().cloned() else {
        return error_response(V4Error::Unavailable, context.request_id());
    };
    let worker = match domain::worker_by_name(&api, account, &script) {
        Ok(value) => value,
        Err(error) => return platform_error(context.request_id(), &error),
    };
    let repo = WorkerRepository::new(api.storage.db());
    let now = now_ms();
    let versions = match repo.list_versions(account, worker.id) {
        Ok(values) => values
            .into_iter()
            .filter(|version| version.deleted_at_ms.is_none())
            .map(|version| version.id)
            .collect::<Vec<_>>(),
        Err(error) => return platform_error(context.request_id(), &error),
    };
    let loader_namespaces = match open_compute_workers::worker_loader_namespaces(
        api.storage.db(),
        account,
        worker.id,
    ) {
        Ok(value) => value,
        Err(error) => return platform_error(context.request_id(), &error),
    };
    if let Err(error) = api
        .pins
        .fence_many_and_wait(&versions, api.delete_drain_timeout)
        .await
    {
        for version in &versions {
            api.pins.unfence(*version);
        }
        return platform_error(context.request_id(), &error);
    }
    if let Some(cache) = &api.response_cache
        && let Err(error) = cache.purge_worker(account, worker.id, now)
    {
        for version in &versions {
            api.pins.unfence(*version);
        }
        return platform_error(context.request_id(), &error);
    }
    if let Err(error) = repo.delete_worker(account, worker.id, &versions, context.request_id(), now)
    {
        for version in &versions {
            api.pins.unfence(*version);
        }
        return platform_error(context.request_id(), &error);
    }
    if let Ok(observability) = api.observability() {
        observability.revoke_worker_tails(account, worker.id);
    }
    api.traffic.remove(worker.id);
    for version in versions {
        api.pins.retire_fence(version);
    }
    if let Err(error) = api
        .transport
        .revoke_worker_loaders(&loader_namespaces)
        .await
    {
        return platform_error(context.request_id(), &error);
    }
    success_response(context, ())
}

#[derive(Serialize)]
struct ScriptSettings {
    logpush: bool,
    observability: ObservabilitySettings,
    tags: Vec<String>,
    tail_consumers: Vec<()>,
}

impl ScriptSettings {
    fn from_persisted(value: &open_compute_storage::WorkerObservabilitySettings) -> Self {
        Self {
            logpush: false,
            observability: ObservabilitySettings {
                enabled: value.enabled,
                head_sampling_rate: value.head_sampling_rate,
                logs: ObservabilityLogsSettings {
                    enabled: value.logs_enabled,
                    head_sampling_rate: value.logs_head_sampling_rate,
                    invocation_logs: value.invocation_logs,
                    persist: value.persist,
                    destinations: Vec::new(),
                },
                traces: ObservabilityTracesSettings {
                    enabled: false,
                    persist: false,
                    destinations: Vec::new(),
                },
            },
            tags: Vec::new(),
            tail_consumers: Vec::new(),
        }
    }
}

#[derive(Serialize)]
struct ObservabilitySettings {
    enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    head_sampling_rate: Option<f64>,
    logs: ObservabilityLogsSettings,
    traces: ObservabilityTracesSettings,
}

#[derive(Serialize)]
struct ObservabilityLogsSettings {
    enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    head_sampling_rate: Option<f64>,
    invocation_logs: bool,
    persist: bool,
    destinations: Vec<()>,
}

#[derive(Serialize)]
struct ObservabilityTracesSettings {
    enabled: bool,
    persist: bool,
    destinations: Vec<()>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ScriptSettingsPatch {
    logpush: Option<bool>,
    observability: Option<ObservabilityPatch>,
    tags: Option<Vec<String>>,
    tail_consumers: Option<Vec<serde_json::Value>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ObservabilityPatch {
    enabled: Option<bool>,
    head_sampling_rate: Option<f64>,
    logs: Option<ObservabilityLogsPatch>,
    traces: Option<ObservabilityTracesPatch>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ObservabilityLogsPatch {
    enabled: Option<bool>,
    head_sampling_rate: Option<f64>,
    invocation_logs: Option<bool>,
    persist: Option<bool>,
    destinations: Option<Vec<serde_json::Value>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ObservabilityTracesPatch {
    enabled: Option<bool>,
    persist: Option<bool>,
    head_sampling_rate: Option<f64>,
    destinations: Option<Vec<serde_json::Value>>,
}

pub(super) async fn get_script_settings(
    State(state): State<HttpState>,
    Path(path): Path<(String, String)>,
    request: Request,
) -> Response {
    let (context, worker) = match settings_context(&state, &path, &request, V4Permission::Read) {
        Ok(value) => value,
        Err(response) => return response.into_response(),
    };
    let Some(api) = state.worker_api() else {
        return error_response(V4Error::Unavailable, context.request_id());
    };
    match WorkerRepository::new(api.storage.db())
        .get_observability_settings(worker.account_id, worker.id)
    {
        Ok(value) => success_response(context, ScriptSettings::from_persisted(&value)),
        Err(error) => platform_error(context.request_id(), &error),
    }
}

pub(super) async fn patch_script_settings(
    State(state): State<HttpState>,
    Path(path): Path<(String, String)>,
    request: Request,
) -> Response {
    let (context, worker) =
        match settings_context(&state, &path, &request, V4Permission::ProductWrite) {
            Ok(value) => value,
            Err(response) => return response.into_response(),
        };
    let patch = match json_body::<ScriptSettingsPatch>(request).await {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    if patch.logpush.is_some_and(|value| value)
        || patch.tags.as_ref().is_some_and(|value| !value.is_empty())
        || patch
            .tail_consumers
            .as_ref()
            .is_some_and(|value| !value.is_empty())
    {
        return error_response(V4Error::Unsupported, context.request_id());
    }
    let Some(api) = state.worker_api() else {
        return error_response(V4Error::Unavailable, context.request_id());
    };
    let repo = WorkerRepository::new(api.storage.db());
    let current = match repo.get_observability_settings(worker.account_id, worker.id) {
        Ok(value) => value,
        Err(error) => return platform_error(context.request_id(), &error),
    };
    let replacement = match merge_observability(&current, patch.observability) {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    let now = now_ms();
    if current.enabled == replacement.enabled
        && current.head_sampling_rate == replacement.head_sampling_rate
        && current.logs_enabled == replacement.logs_enabled
        && current.logs_head_sampling_rate == replacement.logs_head_sampling_rate
        && current.invocation_logs == replacement.invocation_logs
        && current.persist == replacement.persist
    {
        return success_response(context, ScriptSettings::from_persisted(&current));
    }
    match repo.update_observability_settings(
        worker.account_id,
        worker.id,
        &replacement,
        context.request_id(),
        now,
    ) {
        Ok(value) => success_response(context, ScriptSettings::from_persisted(&value)),
        Err(error) => platform_error(context.request_id(), &error),
    }
}

fn merge_observability(
    current: &open_compute_storage::WorkerObservabilitySettings,
    patch: Option<ObservabilityPatch>,
) -> Result<UpdateWorkerObservabilitySettings, V4Error> {
    let Some(patch) = patch else {
        return Ok(UpdateWorkerObservabilitySettings {
            enabled: current.enabled,
            head_sampling_rate: current.head_sampling_rate,
            logs_enabled: current.logs_enabled,
            logs_head_sampling_rate: current.logs_head_sampling_rate,
            invocation_logs: current.invocation_logs,
            persist: current.persist,
        });
    };
    validate_rate(
        patch.head_sampling_rate,
        "/observability/head_sampling_rate",
    )?;
    if let Some(traces) = &patch.traces
        && (traces.enabled.is_some_and(|value| value)
            || traces.persist.is_some_and(|value| value)
            || traces.head_sampling_rate.is_some()
            || traces
                .destinations
                .as_ref()
                .is_some_and(|values| !values.is_empty()))
    {
        return Err(V4Error::Unsupported);
    }
    if patch
        .logs
        .as_ref()
        .and_then(|logs| logs.destinations.as_ref())
        .is_some_and(|values| !values.is_empty())
    {
        return Err(V4Error::Unsupported);
    }
    let logs = patch.logs;
    let logs_rate = logs.as_ref().and_then(|value| value.head_sampling_rate);
    validate_rate(logs_rate, "/observability/logs/head_sampling_rate")?;
    Ok(UpdateWorkerObservabilitySettings {
        enabled: patch.enabled.unwrap_or(current.enabled),
        head_sampling_rate: patch.head_sampling_rate.or(current.head_sampling_rate),
        logs_enabled: logs
            .as_ref()
            .and_then(|value| value.enabled)
            .unwrap_or(current.logs_enabled),
        logs_head_sampling_rate: logs_rate.or(current.logs_head_sampling_rate),
        invocation_logs: logs
            .as_ref()
            .and_then(|value| value.invocation_logs)
            .unwrap_or(current.invocation_logs),
        persist: logs
            .as_ref()
            .and_then(|value| value.persist)
            .unwrap_or(current.persist),
    })
}

fn validate_rate(value: Option<f64>, pointer: &'static str) -> Result<(), V4Error> {
    if value.is_none_or(|rate| rate.is_finite() && (0.0..=1.0).contains(&rate)) {
        Ok(())
    } else {
        Err(V4Error::InvalidField(pointer))
    }
}

#[derive(Serialize)]
struct VersionSettings {
    bindings: Vec<serde_json::Value>,
    compatibility_date: String,
    compatibility_flags: Vec<String>,
    usage_model: &'static str,
    logpush: bool,
    placement: BTreeMap<String, String>,
    tail_consumers: Vec<()>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct VersionSettingsPatch {
    compatibility_date: Option<String>,
    compatibility_flags: Option<Vec<String>>,
    bindings: Option<Vec<serde_json::Value>>,
    cache_options: Option<serde_json::Value>,
    exports: Option<serde_json::Value>,
    migrations: Option<serde_json::Value>,
    annotations: Option<BTreeMap<String, String>>,
    logpush: Option<bool>,
    observability: Option<serde_json::Value>,
    placement: Option<serde_json::Value>,
    tags: Option<Vec<String>>,
    tail_consumers: Option<Vec<serde_json::Value>>,
    usage_model: Option<String>,
}

pub(super) async fn get_settings(
    State(state): State<HttpState>,
    Path((account, script)): Path<(String, String)>,
    request: Request,
) -> Response {
    let context = match authorize(&request, V4Permission::Read) {
        Ok(value) => value,
        Err(response) => return response.into_response(),
    };
    let result = active_snapshot(&state, &account, &script).and_then(|(_, snapshot)| {
        let api = worker_api(&state)?;
        let authority = state.cloudflare_v4_account().ok_or(V4Error::Unavailable)?;
        Ok(VersionSettings {
            bindings: super::projection::public_bindings(api, authority, &snapshot)
                .map_err(|error| V4Error::from(&error))?,
            compatibility_date: snapshot.version.compatibility_date,
            compatibility_flags: snapshot.version.compatibility_flags,
            usage_model: "standard",
            logpush: false,
            placement: BTreeMap::new(),
            tail_consumers: Vec::new(),
        })
    });
    respond(context, result)
}

pub(super) async fn patch_settings(
    State(state): State<HttpState>,
    Path((account, script)): Path<(String, String)>,
    request: Request,
) -> Response {
    let context = match authorize(&request, V4Permission::ProductWrite) {
        Ok(value) => value,
        Err(response) => return response.into_response(),
    };
    let Ok(multipart) = Multipart::from_request(request, &state).await else {
        return error_response(V4Error::InvalidRequest, context.request_id());
    };
    let patch = match read_settings_part(multipart).await {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    let (_worker, snapshot) = match active_snapshot(&state, &account, &script) {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    let exact_date = patch
        .compatibility_date
        .as_deref()
        .is_none_or(|value| value == snapshot.version.compatibility_date);
    let exact_flags = patch
        .compatibility_flags
        .as_ref()
        .is_none_or(|value| *value == snapshot.version.compatibility_flags);
    let no_unsupported = patch.bindings.as_ref().is_none_or(Vec::is_empty)
        && patch.cache_options.is_none()
        && patch.exports.is_none()
        && patch.migrations.is_none()
        && patch.annotations.as_ref().is_none_or(BTreeMap::is_empty)
        && !patch.logpush.unwrap_or(false)
        && patch
            .observability
            .as_ref()
            .is_none_or(serde_json::Value::is_null)
        && patch
            .placement
            .as_ref()
            .is_none_or(|value| value.as_object().is_some_and(serde_json::Map::is_empty))
        && patch.tags.as_ref().is_none_or(Vec::is_empty)
        && patch.tail_consumers.as_ref().is_none_or(Vec::is_empty)
        && patch
            .usage_model
            .as_deref()
            .is_none_or(|value| value == "standard");
    if !exact_date || !exact_flags {
        return error_response(V4Error::Unsupported, context.request_id());
    }
    if !no_unsupported {
        return error_response(V4Error::Unsupported, context.request_id());
    }
    success_response(
        context,
        VersionSettings {
            bindings: match super::projection::public_bindings(
                match worker_api(&state) {
                    Ok(value) => value,
                    Err(error) => return error_response(error, context.request_id()),
                },
                match state.cloudflare_v4_account() {
                    Some(value) => value,
                    None => {
                        return error_response(V4Error::Unavailable, context.request_id());
                    }
                },
                &snapshot,
            ) {
                Ok(value) => value,
                Err(error) => return platform_error(context.request_id(), &error),
            },
            compatibility_date: snapshot.version.compatibility_date,
            compatibility_flags: snapshot.version.compatibility_flags,
            usage_model: "standard",
            logpush: false,
            placement: BTreeMap::new(),
            tail_consumers: Vec::new(),
        },
    )
}

async fn read_settings_part(mut multipart: Multipart) -> Result<VersionSettingsPatch, V4Error> {
    let field = multipart
        .next_field()
        .await
        .map_err(|_| V4Error::InvalidRequest)?
        .ok_or(V4Error::InvalidRequest)?;
    if field.name() != Some("settings") || field.content_type() != Some("application/json") {
        return Err(V4Error::InvalidRequest);
    }
    let bytes = field.bytes().await.map_err(|_| V4Error::InvalidRequest)?;
    if bytes.len() > 1024 * 1024
        || multipart
            .next_field()
            .await
            .map_err(|_| V4Error::InvalidRequest)?
            .is_some()
    {
        return Err(V4Error::InvalidRequest);
    }
    serde_json::from_slice(&bytes).map_err(|_| V4Error::InvalidRequest)
}

mod schedules;
mod secrets;
mod subdomain;

pub(super) use schedules::{get_schedules, put_schedules};
#[cfg(test)]
use secrets::SecretBody;
pub(super) use secrets::{delete_secret, get_secret, list_secrets, patch_secrets_bulk, put_secret};
pub(super) use subdomain::{delete_subdomain, get_subdomain, post_subdomain};

fn settings_read_context(
    state: &HttpState,
    path: &(String, String),
    request: &Request,
    permission: V4Permission,
) -> Result<crate::cloudflare_v4::V4RequestContext, HttpError> {
    settings_context(state, path, request, permission).map(|(context, _)| context)
}

fn settings_context(
    state: &HttpState,
    (account, script): &(String, String),
    request: &Request,
    permission: V4Permission,
) -> Result<(crate::cloudflare_v4::V4RequestContext, WorkerRecord), HttpError> {
    let context = authorize(request, permission)?;
    let account = domain::resolve_account(state, account)
        .map_err(|error| error_response(error, context.request_id()))?;
    let api = worker_api(state).map_err(|error| error_response(error, context.request_id()))?;
    let worker = domain::worker_by_name(api, account, script)
        .map_err(|error| platform_error(context.request_id(), &error))?;
    Ok((context, worker))
}

fn active_snapshot(
    state: &HttpState,
    account: &str,
    script: &str,
) -> Result<(WorkerRecord, VersionSnapshot), V4Error> {
    let account = domain::resolve_account(state, account)?;
    let api = worker_api(state)?;
    let worker =
        domain::worker_by_name(api, account, script).map_err(|error| V4Error::from(&error))?;
    let version = worker.active_version_id.ok_or(V4Error::Conflict)?;
    let snapshot = WorkerRepository::new(api.storage.db())
        .version_snapshot(account, worker.id, version, false)
        .map_err(|error| V4Error::from(&error))?;
    Ok((worker, snapshot))
}

async fn mutate(
    state: &HttpState,
    account: &str,
    script: &str,
    secret_updates: BTreeMap<String, Option<SecretString>>,
    crons: Option<Vec<String>>,
    request_id: open_compute_core::RequestId,
) -> Result<(), PlatformError> {
    let account = domain::resolve_account(state, account).map_err(v4_platform_error)?;
    let api = state.worker_api().ok_or_else(unavailable)?;
    let worker = domain::worker_by_name(api, account, script)?;
    match domain::clone_active(
        api,
        account,
        &worker,
        secret_updates,
        crons,
        request_id,
        now_ms(),
    )
    .await?
    {
        CreateVersionOutcome::Applied(_) => Ok(()),
        CreateVersionOutcome::Replay(_) => Err(PlatformError::new(
            ErrorCode::IdempotencyConflict,
            "mutation request identifier was replayed",
        )),
    }
}

fn v4_platform_error(error: V4Error) -> PlatformError {
    PlatformError::new(
        match error {
            V4Error::NotFound => ErrorCode::AccountNotFound,
            V4Error::Unavailable => ErrorCode::PlatformUnavailable,
            _ => ErrorCode::ConfigInvalid,
        },
        "v4 request authority is unavailable",
    )
}

fn unavailable() -> PlatformError {
    PlatformError::new(ErrorCode::PlatformUnavailable, "Worker API is unavailable")
}

fn delete_force_query(query: Option<&str>) -> Result<bool, V4Error> {
    let Some(query) = query else {
        return Ok(false);
    };
    if query.is_empty() {
        return Ok(false);
    }
    let mut pairs = url::form_urlencoded::parse(query.as_bytes());
    let Some((name, value)) = pairs.next() else {
        return Ok(false);
    };
    if name != "force" || pairs.next().is_some() {
        return Err(V4Error::InvalidRequest);
    }
    match value.as_ref() {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err(V4Error::InvalidRequest),
    }
}

#[cfg(test)]
mod tests;
