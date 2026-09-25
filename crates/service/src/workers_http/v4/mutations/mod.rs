//! Immutable Worker settings, secret, schedule, and deletion adapters.

use super::domain;
use super::handlers::{authorize, json_body, now_ms, platform_error, respond, worker_api};
use crate::cloudflare_v4::{HttpError, V4Error, V4Permission, error_response, success_response};
use crate::http::HttpState;
use axum::extract::{FromRequest, Multipart, Path, Request, State};
use axum::response::Response;
use open_compute_core::{ErrorCode, PlatformError, SecretString};
use open_compute_storage::{
    CronRepository, EffectiveResourceLimits, UpdateWorkerObservabilitySettings, VersionSnapshot,
    WorkerRecord, WorkerRepository,
};
use open_compute_workers::{CreateVersionOutcome, RuntimeValidator};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

mod deletion;

#[cfg(test)]
use deletion::delete_force_query;
pub(super) use deletion::delete_script;

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
        .get_observability_settings(worker.instance_id, worker.id)
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
    let current = match repo.get_observability_settings(worker.instance_id, worker.id) {
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
    let worker = match repo.get_worker(worker.instance_id, worker.id) {
        Ok(value) => value,
        Err(error) => return platform_error(context.request_id(), &error),
    };
    let mut revoked_generation = None;
    if let Some(active) = worker.active_version_id {
        match open_compute_workers::version_has_worker_loader(api.storage.db(), active) {
            Ok(true) => {
                let Some(generation) = api.transport.current_generation() else {
                    let error = PlatformError::new(
                        ErrorCode::RuntimeUnavailable,
                        "runtime generation is unavailable for Loader revocation",
                    );
                    return platform_error(context.request_id(), &error);
                };
                let prefix = open_compute_workers::worker_loader_generation_prefix(
                    worker.instance_id,
                    worker.id,
                    worker.route_generation,
                );
                if let Err(error) = api
                    .transport
                    .revoke_worker_loader_prefix(prefix, generation)
                    .await
                {
                    return platform_error(context.request_id(), &error);
                }
                revoked_generation = Some(generation);
            }
            Ok(false) => {}
            Err(error) => return platform_error(context.request_id(), &error),
        }
    }
    match repo.update_observability_settings(
        worker.instance_id,
        worker.id,
        worker.route_generation,
        &replacement,
        context.request_id(),
        now,
    ) {
        Ok(value) => success_response(context, ScriptSettings::from_persisted(&value)),
        Err(error) => {
            if let Some(generation) = revoked_generation
                && let Err(recovery) = api
                    .transport
                    .recover_worker_loader_revocation(generation)
                    .await
            {
                return platform_error(context.request_id(), &recovery);
            }
            platform_error(context.request_id(), &error)
        }
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
    annotations: BTreeMap<String, String>,
    bindings: Vec<serde_json::Value>,
    compatibility_date: String,
    compatibility_flags: Vec<String>,
    limits: super::model::WorkerUploadResourceLimits,
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
    #[serde(
        default,
        deserialize_with = "super::model::deserialize_optional_resource_limits"
    )]
    limits: Option<super::model::WorkerUploadResourceLimits>,
    bindings: Option<Vec<super::model::WorkerUploadBinding>>,
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

fn normalize_patch_annotations(
    mut annotations: BTreeMap<String, String>,
) -> Result<BTreeMap<String, String>, V4Error> {
    if annotations.len() > 2
        || annotations.iter().any(|(key, value)| {
            !matches!(key.as_str(), "workers/message" | "workers/tag")
                || value.chars().any(char::is_control)
                || (key == "workers/tag" && value.len() > 100)
        })
    {
        return Err(V4Error::InvalidRequest);
    }
    if let Some(message) = annotations.get_mut("workers/message") {
        let mut end = message.len().min(1_000);
        while !message.is_char_boundary(end) {
            end -= 1;
        }
        message.truncate(end);
    }
    Ok(annotations)
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
    let result = settings_snapshot(&state, &account, &script).and_then(|(_, snapshot)| {
        let api = worker_api(&state)?;
        let authority = state.v4_instance_context().ok_or(V4Error::Unavailable)?;
        Ok(VersionSettings {
            annotations: snapshot.annotations.clone(),
            bindings: super::projection::public_bindings(api, authority, &snapshot)
                .map_err(|error| V4Error::from(&error))?,
            compatibility_date: snapshot.version.compatibility_date,
            compatibility_flags: snapshot.version.compatibility_flags,
            limits: public_limits(snapshot.version.resource_limits),
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
    let (worker, snapshot) = match settings_snapshot(&state, &account, &script) {
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
    let annotation_change = patch.annotations.is_some();
    let annotations = match normalize_patch_annotations(patch.annotations.unwrap_or_default()) {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    let no_unsupported = patch.cache_options.is_none()
        && patch.exports.is_none()
        && patch.migrations.is_none()
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
    if let Some(bindings) = &patch.bindings {
        let mut names = std::collections::BTreeSet::new();
        for binding in bindings {
            if !names.insert(binding.name()) {
                return error_response(V4Error::InvalidRequest, context.request_id());
            }
            if matches!(
                binding,
                super::model::WorkerUploadBinding::Assets { .. }
                    | super::model::WorkerUploadBinding::WasmModule { .. }
                    | super::model::WorkerUploadBinding::TextBlob { .. }
                    | super::model::WorkerUploadBinding::DataBlob { .. }
            ) {
                return error_response(V4Error::Unsupported, context.request_id());
            }
        }
    }
    if patch.limits.is_some() || patch.bindings.is_some() || annotation_change {
        let limits = patch
            .limits
            .unwrap_or(super::model::WorkerUploadResourceLimits {
                cpu_ms: None,
                sub_requests: None,
            });
        let replacement = match EffectiveResourceLimits::new(
            limits
                .cpu_ms
                .unwrap_or(snapshot.version.resource_limits.cpu_ms),
            limits
                .sub_requests
                .unwrap_or(snapshot.version.resource_limits.sub_requests),
        ) {
            Ok(value) => value,
            Err(error) => return platform_error(context.request_id(), &error),
        };
        let api = match worker_api(&state) {
            Ok(value) => value,
            Err(error) => return error_response(error, context.request_id()),
        };
        let Some(authority) = state.v4_instance_context() else {
            return error_response(V4Error::Unavailable, context.request_id());
        };
        let bindings = patch.bindings.map(|bindings| (authority, bindings));
        match domain::clone_version(
            api,
            &worker,
            domain::CloneVersionOptions {
                source_version: snapshot.version.id,
                deployment_source: None,
                secret_updates: BTreeMap::new(),
                crons: None,
                resource_limits: Some(replacement),
                binding_patch: bindings,
                annotations,
                request_id: context.request_id(),
                now_ms: now_ms(),
            },
        )
        .await
        {
            Ok(CreateVersionOutcome::Applied(_)) => {}
            Ok(CreateVersionOutcome::Replay(_)) => {
                return error_response(V4Error::Conflict, context.request_id());
            }
            Err(error) => return platform_error(context.request_id(), &error),
        }
        let (_, current) = match settings_snapshot(&state, &account, &script) {
            Ok(value) => value,
            Err(error) => return error_response(error, context.request_id()),
        };
        return settings_response(&state, context, current);
    }
    settings_response(&state, context, snapshot)
}

fn settings_response(
    state: &HttpState,
    context: crate::cloudflare_v4::V4RequestContext,
    snapshot: VersionSnapshot,
) -> Response {
    success_response(
        context,
        VersionSettings {
            annotations: snapshot.annotations.clone(),
            bindings: match super::projection::public_bindings(
                match worker_api(state) {
                    Ok(value) => value,
                    Err(error) => return error_response(error, context.request_id()),
                },
                match state.v4_instance_context() {
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
            limits: public_limits(snapshot.version.resource_limits),
            usage_model: "standard",
            logpush: false,
            placement: BTreeMap::new(),
            tail_consumers: Vec::new(),
        },
    )
}

fn public_limits(value: EffectiveResourceLimits) -> super::model::WorkerUploadResourceLimits {
    super::model::WorkerUploadResourceLimits {
        cpu_ms: Some(value.cpu_ms),
        sub_requests: Some(value.sub_requests),
    }
}

async fn read_settings_part(mut multipart: Multipart) -> Result<VersionSettingsPatch, V4Error> {
    let mut fields = Vec::new();
    let mut size = 0;
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|_| V4Error::InvalidRequest)?
    {
        if field.name() != Some("settings") && field.file_name().is_some() {
            return Err(V4Error::InvalidRequest);
        }
        let name = field.name().ok_or(V4Error::InvalidRequest)?.to_owned();
        let content_type = field.content_type().map(str::to_owned);
        let bytes = field.bytes().await.map_err(|_| V4Error::InvalidRequest)?;
        size += bytes.len() + name.len();
        if size > 1024 * 1024 || fields.len() >= 1024 {
            return Err(V4Error::InvalidRequest);
        }
        fields.push((name, content_type, bytes));
    }
    if fields.is_empty() {
        return Err(V4Error::InvalidRequest);
    }
    if fields.len() == 1 && fields[0].0 == "settings" {
        let (_, content_type, bytes) = fields.pop().ok_or(V4Error::InvalidRequest)?;
        if content_type
            .as_deref()
            .and_then(|value| value.split(';').next())
            != Some("application/json")
        {
            return Err(V4Error::InvalidRequest);
        }
        return serde_json::from_slice(&bytes).map_err(|_| V4Error::InvalidRequest);
    }
    let mut settings = serde_json::Map::new();
    let mut bindings: Vec<serde_json::Map<String, serde_json::Value>> = Vec::new();
    for (name, _, bytes) in fields {
        let value = String::from_utf8(bytes.to_vec()).map_err(|_| V4Error::InvalidRequest)?;
        let path = name
            .strip_prefix("settings[")
            .and_then(|name| name.strip_suffix(']'))
            .ok_or(V4Error::InvalidRequest)?;
        if let Some(key) = path.strip_prefix("bindings][][") {
            if key.is_empty() || key.contains(['[', ']']) {
                return Err(V4Error::InvalidRequest);
            }
            if bindings
                .last()
                .is_none_or(|binding| binding.contains_key(key))
            {
                bindings.push(serde_json::Map::new());
            }
            let binding = bindings.last_mut().ok_or(V4Error::InvalidRequest)?;
            binding.insert(key.to_owned(), value.into());
        } else if let Some((parent, key)) = path.split_once("][") {
            if parent.is_empty() || key.is_empty() || key.contains(['[', ']']) {
                return Err(V4Error::InvalidRequest);
            }
            let entry = settings
                .entry(parent)
                .or_insert_with(|| serde_json::json!({}));
            let object = entry.as_object_mut().ok_or(V4Error::InvalidRequest)?;
            let value = if parent == "limits" && matches!(key, "cpu_ms" | "subrequests") {
                serde_json::Value::from(value.parse::<u64>().map_err(|_| V4Error::InvalidRequest)?)
            } else {
                value.into()
            };
            if object.insert(key.to_owned(), value).is_some() {
                return Err(V4Error::InvalidRequest);
            }
        } else if let Some(key) = path.strip_suffix("][") {
            let entry = settings.entry(key).or_insert_with(|| serde_json::json!([]));
            entry
                .as_array_mut()
                .ok_or(V4Error::InvalidRequest)?
                .push(value.into());
        } else if settings.insert(path.to_owned(), value.into()).is_some() {
            return Err(V4Error::InvalidRequest);
        }
    }
    if !bindings.is_empty() {
        settings.insert(
            "bindings".to_owned(),
            bindings
                .into_iter()
                .map(serde_json::Value::Object)
                .collect(),
        );
    }
    serde_json::from_value(serde_json::Value::Object(settings)).map_err(|_| V4Error::InvalidRequest)
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
    let account = domain::resolve_instance(state, account)
        .map_err(|error| error_response(error, context.request_id()))?;
    let api = worker_api(state).map_err(|error| error_response(error, context.request_id()))?;
    let worker = domain::worker_by_name(api, account, script)
        .map_err(|error| platform_error(context.request_id(), &error))?;
    Ok((context, worker))
}

fn settings_snapshot(
    state: &HttpState,
    account: &str,
    script: &str,
) -> Result<(WorkerRecord, VersionSnapshot), V4Error> {
    let account = domain::resolve_instance(state, account)?;
    let api = worker_api(state)?;
    let worker =
        domain::worker_by_name(api, account, script).map_err(|error| V4Error::from(&error))?;
    let repo = WorkerRepository::new(api.storage.db());
    let version = repo
        .list_versions(account, worker.id)
        .map_err(|error| V4Error::from(&error))?
        .into_iter()
        .find(|version| version.state == open_compute_storage::VersionState::Ready)
        .ok_or(V4Error::Conflict)?;
    let snapshot = repo
        .version_snapshot(account, worker.id, version.id, false)
        .map_err(|error| V4Error::from(&error))?;
    Ok((worker, snapshot))
}

fn active_snapshot(
    state: &HttpState,
    account: &str,
    script: &str,
) -> Result<(WorkerRecord, VersionSnapshot), V4Error> {
    let account = domain::resolve_instance(state, account)?;
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
    let (worker, snapshot) =
        settings_snapshot(state, account, script).map_err(v4_platform_error)?;
    let api = state.worker_api().ok_or_else(unavailable)?;
    match domain::clone_version(
        api,
        &worker,
        domain::CloneVersionOptions {
            source_version: snapshot.version.id,
            deployment_source: Some(open_compute_storage::DeploymentSource::VersionsApi),
            secret_updates,
            crons,
            resource_limits: None,
            binding_patch: None,
            annotations: BTreeMap::new(),
            request_id,
            now_ms: now_ms(),
        },
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
            V4Error::NotFound => ErrorCode::InstanceNotFound,
            V4Error::Unavailable => ErrorCode::PlatformUnavailable,
            _ => ErrorCode::ConfigInvalid,
        },
        "v4 request authority is unavailable",
    )
}

fn unavailable() -> PlatformError {
    PlatformError::new(ErrorCode::PlatformUnavailable, "Worker API is unavailable")
}

#[cfg(test)]
mod tests;
