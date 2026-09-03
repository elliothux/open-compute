//! Immutable Worker settings, secret, schedule, and deletion adapters.

use super::domain;
use super::handlers::{
    authorize, json_body, now_ms, platform_error, respond, timestamp, worker_api,
};
use crate::cloudflare_v4::{HttpError, V4Error, V4Permission, error_response, success_response};
use crate::http::HttpState;
use axum::extract::{FromRequest, Multipart, Path, Request, State};
use axum::response::Response;
use open_compute_core::{ErrorCode, PlatformError, SecretString};
use open_compute_storage::{CronRepository, VersionSnapshot, WorkerRecord, WorkerRepository};
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
    let now = match now_ms() {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    let versions = match repo.list_versions(account, worker.id) {
        Ok(values) => values
            .into_iter()
            .filter(|version| version.deleted_at_ms.is_none())
            .map(|version| version.id)
            .collect::<Vec<_>>(),
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
    api.traffic.remove(worker.id);
    for version in versions {
        api.pins.retire_fence(version);
    }
    success_response(context, ())
}

#[derive(Serialize)]
struct ScriptSettings {
    logpush: bool,
    observability: DisabledObservability,
    tags: Vec<String>,
    tail_consumers: Vec<()>,
}

impl ScriptSettings {
    const fn disabled() -> Self {
        Self {
            logpush: false,
            observability: DisabledObservability { enabled: false },
            tags: Vec::new(),
            tail_consumers: Vec::new(),
        }
    }
}

#[derive(Serialize)]
struct DisabledObservability {
    enabled: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ScriptSettingsPatch {
    logpush: Option<bool>,
    observability: Option<serde_json::Value>,
    tags: Option<Vec<String>>,
    tail_consumers: Option<Vec<serde_json::Value>>,
}

pub(super) async fn get_script_settings(
    State(state): State<HttpState>,
    Path(path): Path<(String, String)>,
    request: Request,
) -> Response {
    settings_read_context(&state, &path, &request, V4Permission::Read)
        .map_or_else(HttpError::into_response, |context| {
            success_response(context, ScriptSettings::disabled())
        })
}

pub(super) async fn patch_script_settings(
    State(state): State<HttpState>,
    Path(path): Path<(String, String)>,
    request: Request,
) -> Response {
    let context = match settings_read_context(&state, &path, &request, V4Permission::ProductWrite) {
        Ok(value) => value,
        Err(response) => return response.into_response(),
    };
    let patch = match json_body::<ScriptSettingsPatch>(request).await {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    if patch.logpush.is_some_and(|value| value)
        || patch
            .observability
            .as_ref()
            .is_some_and(|value| !disabled_observability(value))
        || patch.tags.as_ref().is_some_and(|value| !value.is_empty())
        || patch
            .tail_consumers
            .as_ref()
            .is_some_and(|value| !value.is_empty())
    {
        return error_response(V4Error::Unsupported, context.request_id());
    }
    success_response(context, ScriptSettings::disabled())
}

fn disabled_observability(value: &serde_json::Value) -> bool {
    let Some(object) = value.as_object() else {
        return false;
    };
    object.len() == 1 && object.get("enabled") == Some(&serde_json::Value::Bool(false))
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
    limits: Option<serde_json::Value>,
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
        && patch.limits.is_none()
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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SecretBody {
    name: String,
    #[serde(rename = "type")]
    kind: String,
    text: Option<SecretString>,
    format: Option<String>,
    algorithm: Option<serde_json::Value>,
    usages: Option<Vec<String>>,
    key_base64: Option<SecretString>,
    key_jwk: Option<serde_json::Value>,
}

impl SecretBody {
    fn text(self) -> Result<(String, SecretString), V4Error> {
        if self.kind == "secret_key" {
            return Err(V4Error::Unsupported);
        }
        if self.kind != "secret_text"
            || self.format.is_some()
            || self.algorithm.is_some()
            || self.usages.is_some()
            || self.key_base64.is_some()
            || self.key_jwk.is_some()
        {
            return Err(V4Error::InvalidRequest);
        }
        Ok((self.name, self.text.ok_or(V4Error::InvalidRequest)?))
    }
}

#[derive(Clone, Serialize)]
struct SecretItem {
    name: String,
    #[serde(rename = "type")]
    kind: &'static str,
}

pub(super) async fn list_secrets(
    State(state): State<HttpState>,
    Path((account, script)): Path<(String, String)>,
    request: Request,
) -> Response {
    let context = match authorize(&request, V4Permission::Read) {
        Ok(value) => value,
        Err(response) => return response.into_response(),
    };
    let result = active_snapshot(&state, &account, &script).map(|(_, snapshot)| {
        snapshot
            .secrets
            .keys()
            .map(|name| SecretItem {
                name: name.clone(),
                kind: "secret_text",
            })
            .collect::<Vec<_>>()
    });
    respond(context, result)
}

pub(super) async fn put_secret(
    State(state): State<HttpState>,
    Path((account, script)): Path<(String, String)>,
    request: Request,
) -> Response {
    let context = match authorize(&request, V4Permission::ProductWrite) {
        Ok(value) => value,
        Err(response) => return response.into_response(),
    };
    let secret = match json_body::<SecretBody>(request)
        .await
        .and_then(SecretBody::text)
    {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    let item = SecretItem {
        name: secret.0.clone(),
        kind: "secret_text",
    };
    let mut updates = BTreeMap::new();
    updates.insert(secret.0, Some(secret.1));
    match mutate(
        &state,
        &account,
        &script,
        updates,
        None,
        context.request_id(),
    )
    .await
    {
        Ok(()) => success_response(context, item),
        Err(error) => platform_error(context.request_id(), &error),
    }
}

pub(super) async fn get_secret(
    State(state): State<HttpState>,
    Path((account, script, secret)): Path<(String, String, String)>,
    request: Request,
) -> Response {
    let context = match authorize(&request, V4Permission::Read) {
        Ok(value) => value,
        Err(response) => return response.into_response(),
    };
    let result = active_snapshot(&state, &account, &script).and_then(|(_, snapshot)| {
        snapshot
            .secrets
            .contains_key(&secret)
            .then_some(SecretItem {
                name: secret,
                kind: "secret_text",
            })
            .ok_or(V4Error::NotFound)
    });
    respond(context, result)
}

pub(super) async fn delete_secret(
    State(state): State<HttpState>,
    Path((account, script, secret)): Path<(String, String, String)>,
    request: Request,
) -> Response {
    let context = match authorize(&request, V4Permission::ProductWrite) {
        Ok(value) => value,
        Err(response) => return response.into_response(),
    };
    let mut updates = BTreeMap::new();
    updates.insert(secret, None);
    match mutate(
        &state,
        &account,
        &script,
        updates,
        None,
        context.request_id(),
    )
    .await
    {
        Ok(()) => success_response(context, ()),
        Err(error) => platform_error(context.request_id(), &error),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SecretBulkPatch {
    secrets: BTreeMap<String, Option<SecretBody>>,
    version_tags: Option<BTreeMap<String, serde_json::Value>>,
}

pub(super) async fn patch_secrets_bulk(
    State(state): State<HttpState>,
    Path((account, script)): Path<(String, String)>,
    request: Request,
) -> Response {
    let context = match authorize(&request, V4Permission::ProductWrite) {
        Ok(value) => value,
        Err(response) => return response.into_response(),
    };
    let body = match json_body::<SecretBulkPatch>(request).await {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    if body
        .version_tags
        .as_ref()
        .is_some_and(|tags| !tags.is_empty())
    {
        return error_response(V4Error::Unsupported, context.request_id());
    }
    let mut updates = BTreeMap::new();
    for (map_name, value) in body.secrets {
        let value = match value {
            Some(value) => match value.text() {
                Ok((body_name, text)) if body_name == map_name => Some(text),
                Ok(_) => {
                    return error_response(V4Error::InvalidRequest, context.request_id());
                }
                Err(error) => return error_response(error, context.request_id()),
            },
            None => None,
        };
        updates.insert(map_name, value);
    }
    match mutate(
        &state,
        &account,
        &script,
        updates,
        None,
        context.request_id(),
    )
    .await
    {
        Ok(()) => match active_snapshot(&state, &account, &script) {
            Ok((_, snapshot)) => success_response(
                context,
                snapshot
                    .secrets
                    .keys()
                    .map(|name| {
                        (
                            name.clone(),
                            SecretItem {
                                name: name.clone(),
                                kind: "secret_text",
                            },
                        )
                    })
                    .collect::<BTreeMap<_, _>>(),
            ),
            Err(error) => error_response(error, context.request_id()),
        },
        Err(error) => platform_error(context.request_id(), &error),
    }
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Schedule {
    cron: String,
    #[serde(skip_deserializing, skip_serializing_if = "Option::is_none")]
    created_on: Option<String>,
    #[serde(skip_deserializing, skip_serializing_if = "Option::is_none")]
    modified_on: Option<String>,
}

#[derive(Serialize)]
struct Schedules {
    schedules: Vec<Schedule>,
}

pub(super) async fn get_schedules(
    State(state): State<HttpState>,
    Path((account, script)): Path<(String, String)>,
    request: Request,
) -> Response {
    let context = match authorize(&request, V4Permission::Read) {
        Ok(value) => value,
        Err(response) => return response.into_response(),
    };
    let result = active_snapshot(&state, &account, &script).and_then(|(_, snapshot)| {
        CronRepository::new(worker_api(&state)?.storage.db())
            .version_config(snapshot.version.id)
            .map_err(|error| V4Error::from(&error))?
            .declarations
            .into_iter()
            .map(|declaration| {
                let time = timestamp(declaration.created_at_ms)?;
                Ok(Schedule {
                    cron: declaration.expression,
                    created_on: Some(time.clone()),
                    modified_on: Some(time),
                })
            })
            .collect::<Result<Vec<_>, V4Error>>()
            .map(|schedules| Schedules { schedules })
    });
    respond(context, result)
}

pub(super) async fn put_schedules(
    State(state): State<HttpState>,
    Path((account, script)): Path<(String, String)>,
    request: Request,
) -> Response {
    let context = match authorize(&request, V4Permission::ProductWrite) {
        Ok(value) => value,
        Err(response) => return response.into_response(),
    };
    let schedules = match json_body::<Vec<Schedule>>(request).await {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    let crons = schedules.iter().map(|value| value.cron.clone()).collect();
    match mutate(
        &state,
        &account,
        &script,
        BTreeMap::new(),
        Some(crons),
        context.request_id(),
    )
    .await
    {
        Ok(()) => success_response(context, Schedules { schedules }),
        Err(error) => platform_error(context.request_id(), &error),
    }
}

#[derive(Serialize)]
struct Subdomain {
    enabled: bool,
    previews_enabled: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SubdomainPatch {
    enabled: bool,
    previews_enabled: Option<bool>,
}

pub(super) async fn get_subdomain(
    State(state): State<HttpState>,
    Path(path): Path<(String, String)>,
    request: Request,
) -> Response {
    settings_read_context(&state, &path, &request, V4Permission::Read).map_or_else(
        HttpError::into_response,
        |context| {
            success_response(
                context,
                Subdomain {
                    enabled: false,
                    previews_enabled: false,
                },
            )
        },
    )
}

pub(super) async fn post_subdomain(
    State(state): State<HttpState>,
    Path(path): Path<(String, String)>,
    request: Request,
) -> Response {
    let context = match settings_read_context(&state, &path, &request, V4Permission::ProductWrite) {
        Ok(value) => value,
        Err(response) => return response.into_response(),
    };
    let body = match json_body::<SubdomainPatch>(request).await {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    if body.enabled || body.previews_enabled.unwrap_or(false) {
        return error_response(V4Error::Unsupported, context.request_id());
    }
    success_response(
        context,
        Subdomain {
            enabled: false,
            previews_enabled: false,
        },
    )
}

pub(super) async fn delete_subdomain(
    State(state): State<HttpState>,
    Path(path): Path<(String, String)>,
    request: Request,
) -> Response {
    settings_read_context(&state, &path, &request, V4Permission::ProductWrite).map_or_else(
        HttpError::into_response,
        |context| {
            success_response(
                context,
                Subdomain {
                    enabled: false,
                    previews_enabled: false,
                },
            )
        },
    )
}

fn settings_read_context(
    state: &HttpState,
    (account, script): &(String, String),
    request: &Request,
    permission: V4Permission,
) -> Result<crate::cloudflare_v4::V4RequestContext, HttpError> {
    let context = authorize(request, permission)?;
    let account = domain::resolve_account(state, account)
        .map_err(|error| error_response(error, context.request_id()))?;
    let api = worker_api(state).map_err(|error| error_response(error, context.request_id()))?;
    domain::worker_by_name(api, account, script)
        .map_err(|error| platform_error(context.request_id(), &error))?;
    Ok(context)
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
        now_ms().map_err(v4_platform_error)?,
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
