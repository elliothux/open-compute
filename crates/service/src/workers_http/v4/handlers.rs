//! Cloudflare v4 Worker Script, Version, and Deployment handlers.

use super::{domain, multipart, query};
use crate::cloudflare_v4::{
    HttpError, V4Error, V4Permission, V4RequestContext, V4ResultInfo, error_response,
    paginated_response, request_context, success_response,
};
use crate::http::HttpState;
use axum::Router;
use axum::extract::{DefaultBodyLimit, FromRequest, Multipart, Path, Request, State};
use axum::routing::{get, patch};
use open_compute_core::{DeploymentId, PlatformError, RequestId, VersionId};
use open_compute_storage::{
    DeploymentRecord, DeploymentSource, VersionRecord, VersionSnapshot, WorkerRecord,
    WorkerRepository,
};
use open_compute_workers::{CreateVersionOutcome, ProductPromotionRequest};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::str::FromStr;
use std::time::{SystemTime, UNIX_EPOCH};

pub(super) use super::json::json_body;

/// Compose the fixed Wrangler Worker-management subset.
pub(crate) fn router() -> Router<HttpState> {
    Router::new()
        .merge(super::assets::router())
        .merge(super::account_subdomain::router())
        .route(
            "/accounts/{account}/workers/services/{script}",
            get(get_service_metadata),
        )
        .route("/accounts/{account}/workers/scripts", get(list_scripts))
        .route(
            "/accounts/{account}/workers/scripts/{script}",
            get(get_script)
                .put(put_script)
                .delete(super::mutations::delete_script)
                .layer(DefaultBodyLimit::max(multipart::MAX_BODY_BYTES)),
        )
        .route(
            "/accounts/{account}/workers/scripts/{script}/versions",
            get(list_versions)
                .post(post_version)
                .layer(DefaultBodyLimit::max(multipart::MAX_BODY_BYTES)),
        )
        .route(
            "/accounts/{account}/workers/scripts/{script}/versions/{version}",
            get(get_version),
        )
        .route(
            "/accounts/{account}/workers/scripts/{script}/deployments",
            get(list_deployments).post(create_deployment),
        )
        .route(
            "/accounts/{account}/workers/scripts/{script}/deployments/{deployment}",
            get(get_deployment).delete(delete_deployment),
        )
        .route(
            "/accounts/{account}/workers/scripts/{script}/script-settings",
            get(super::mutations::get_script_settings)
                .patch(super::mutations::patch_script_settings),
        )
        .route(
            "/accounts/{account}/workers/scripts/{script}/settings",
            get(super::mutations::get_settings).patch(super::mutations::patch_settings),
        )
        .route(
            "/accounts/{account}/workers/scripts/{script}/secrets",
            get(super::mutations::list_secrets).put(super::mutations::put_secret),
        )
        .route(
            "/accounts/{account}/workers/scripts/{script}/secrets/{secret}",
            get(super::mutations::get_secret).delete(super::mutations::delete_secret),
        )
        .route(
            "/accounts/{account}/workers/scripts/{script}/secrets-bulk",
            patch(super::mutations::patch_secrets_bulk),
        )
        .route(
            "/accounts/{account}/workers/scripts/{script}/schedules",
            get(super::mutations::get_schedules).put(super::mutations::put_schedules),
        )
        .route(
            "/accounts/{account}/workers/scripts/{script}/subdomain",
            get(super::mutations::get_subdomain)
                .post(super::mutations::post_subdomain)
                .delete(super::mutations::delete_subdomain),
        )
}

#[derive(Serialize)]
struct ServiceMetadata {
    default_environment: ServiceEnvironment,
}

#[derive(Serialize)]
struct ServiceEnvironment {
    environment: &'static str,
    script: ServiceScript,
}

#[derive(Serialize)]
struct ServiceScript {
    tag: String,
    tags: Vec<String>,
    last_deployed_from: &'static str,
}

async fn get_service_metadata(
    State(state): State<HttpState>,
    Path((account, script)): Path<(String, String)>,
    request: Request,
) -> axum::response::Response {
    let context = match authorize(&request, V4Permission::Read) {
        Ok(value) => value,
        Err(response) => return response.into_response(),
    };
    let result = (|| {
        let account = domain::resolve_account(&state, &account)?;
        let authority = state.cloudflare_v4_account().ok_or(V4Error::Unavailable)?;
        let api = worker_api(&state)?;
        let worker =
            domain::worker_by_name(api, account, &script).map_err(|error| V4Error::from(&error))?;
        Ok(ServiceMetadata {
            default_environment: ServiceEnvironment {
                environment: "production",
                script: ServiceScript {
                    tag: authority.public_worker_tag(worker.id),
                    tags: Vec::new(),
                    last_deployed_from: "wrangler",
                },
            },
        })
    })();
    respond(context, result)
}

#[derive(Serialize)]
struct ScriptItem {
    id: String,
    created_on: String,
    modified_on: String,
    etag: Option<String>,
}

impl ScriptItem {
    fn from_worker(
        worker: &WorkerRecord,
        version: Option<&VersionRecord>,
    ) -> Result<Self, V4Error> {
        Ok(Self {
            id: worker.name.clone(),
            created_on: timestamp(worker.created_at_ms)?,
            modified_on: timestamp(worker.updated_at_ms)?,
            etag: version.map(|value| hex::encode(value.worker_code_sha256)),
        })
    }
}

#[derive(Serialize)]
struct VersionMetadata {
    created_on: String,
    modified_on: String,
    source: &'static str,
    #[serde(rename = "hasPreview")]
    has_preview: bool,
}

#[derive(Serialize)]
struct VersionItem {
    id: VersionId,
    number: u64,
    metadata: VersionMetadata,
    annotations: BTreeMap<String, String>,
    resources: VersionResources,
}

#[derive(Serialize)]
struct VersionResources {
    bindings: Vec<serde_json::Value>,
    script: VersionScript,
    script_runtime: VersionScriptRuntime,
}

#[derive(Serialize)]
struct VersionScript {
    etag: String,
    last_deployed_from: &'static str,
}

#[derive(Serialize)]
struct VersionScriptRuntime {
    compatibility_date: String,
    compatibility_flags: Vec<String>,
    usage_model: &'static str,
}

impl VersionItem {
    fn from_snapshot(
        api: &crate::workers_http::WorkerApiState,
        authority: &crate::cloudflare_v4::accounts::AccountAuthority,
        snapshot: &VersionSnapshot,
    ) -> Result<Self, V4Error> {
        let version = &snapshot.version;
        let created = timestamp(version.created_at_ms)?;
        Ok(Self {
            id: version.id,
            number: version.version_number,
            metadata: VersionMetadata {
                created_on: created.clone(),
                modified_on: created,
                source: "wrangler",
                has_preview: false,
            },
            annotations: snapshot.annotations.clone(),
            resources: VersionResources {
                bindings: super::projection::public_bindings(api, authority, snapshot)
                    .map_err(|error| V4Error::from(&error))?,
                script: VersionScript {
                    etag: hex::encode(version.worker_code_sha256),
                    last_deployed_from: "wrangler",
                },
                script_runtime: VersionScriptRuntime {
                    compatibility_date: version.compatibility_date.clone(),
                    compatibility_flags: version.compatibility_flags.clone(),
                    usage_model: "standard",
                },
            },
        })
    }
}

#[derive(Serialize)]
struct VersionShort {
    id: VersionId,
    number: u64,
    metadata: VersionMetadata,
    annotations: BTreeMap<String, String>,
}

impl VersionShort {
    fn from_record(
        version: &VersionRecord,
        annotations: BTreeMap<String, String>,
    ) -> Result<Self, V4Error> {
        let created = timestamp(version.created_at_ms)?;
        Ok(Self {
            id: version.id,
            number: version.version_number,
            metadata: VersionMetadata {
                created_on: created.clone(),
                modified_on: created,
                source: "wrangler",
                has_preview: false,
            },
            annotations,
        })
    }
}

#[derive(Serialize)]
struct DeploymentVersion {
    version_id: VersionId,
    percentage: u8,
}

#[derive(Serialize)]
struct DeploymentItem {
    id: DeploymentId,
    source: &'static str,
    strategy: &'static str,
    versions: [DeploymentVersion; 1],
    created_on: String,
    annotations: BTreeMap<String, String>,
}

impl DeploymentItem {
    fn from_record(record: &DeploymentRecord) -> Result<Self, V4Error> {
        Ok(Self {
            id: record.id,
            source: match record.source {
                DeploymentSource::ScriptUpload => "script_upload",
                DeploymentSource::VersionsApi => "api",
                DeploymentSource::Rollback => "rollback",
                DeploymentSource::System => "system",
            },
            strategy: "percentage",
            versions: [DeploymentVersion {
                version_id: record.version_id,
                percentage: 100,
            }],
            created_on: timestamp(record.created_at_ms)?,
            annotations: record.annotations.clone(),
        })
    }
}

#[derive(Serialize)]
struct DeploymentList {
    deployments: Vec<DeploymentItem>,
}

#[derive(Serialize)]
struct VersionList {
    items: Vec<VersionShort>,
}

async fn list_scripts(
    State(state): State<HttpState>,
    Path(account): Path<String>,
    request: Request,
) -> axum::response::Response {
    let context = match authorize(&request, V4Permission::Read) {
        Ok(value) => value,
        Err(response) => return response.into_response(),
    };
    let result = (|| {
        let account = domain::resolve_account(&state, &account)?;
        let api = worker_api(&state)?;
        let repo = WorkerRepository::new(api.storage.db());
        repo.list_workers(account)
            .map_err(|error| V4Error::from(&error))?
            .into_iter()
            .filter(|worker| worker.deleted_at_ms.is_none())
            .map(|worker| {
                let version = worker
                    .active_version_id
                    .map(|id| repo.get_version(account, worker.id, id))
                    .transpose()
                    .map_err(|error| V4Error::from(&error))?;
                ScriptItem::from_worker(&worker, version.as_ref())
            })
            .collect::<Result<Vec<_>, _>>()
    })();
    respond(context, result)
}

async fn get_script(
    State(state): State<HttpState>,
    Path((account, script)): Path<(String, String)>,
    request: Request,
) -> axum::response::Response {
    let context = match authorize(&request, V4Permission::Read) {
        Ok(value) => value,
        Err(response) => return response.into_response(),
    };
    super::download::download_script(state, account, script, request, context).await
}

async fn put_script(
    State(state): State<HttpState>,
    Path((account, script)): Path<(String, String)>,
    request: Request,
) -> axum::response::Response {
    upload(state, account, script, request, true).await
}

async fn post_version(
    State(state): State<HttpState>,
    Path((account, script)): Path<(String, String)>,
    request: Request,
) -> axum::response::Response {
    upload(state, account, script, request, false).await
}

async fn upload(
    state: HttpState,
    account: String,
    script: String,
    request: Request,
    deploy: bool,
) -> axum::response::Response {
    let context = match authorize(&request, V4Permission::ProductWrite) {
        Ok(value) => value,
        Err(response) => return response.into_response(),
    };
    let query = match query::upload(request.uri().query(), deploy) {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    let account = match domain::resolve_account(&state, &account) {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    let Some(account_authority) = state.cloudflare_v4_account().cloned() else {
        return error_response(V4Error::Unavailable, context.request_id());
    };
    let Some(api) = state.worker_api().cloned() else {
        return error_response(V4Error::Unavailable, context.request_id());
    };
    let request = match super::sdk_multipart::normalize_request(request).await {
        Ok(value) => value,
        Err(error) => return platform_error(context.request_id(), &error),
    };
    let Ok(multipart) = Multipart::from_request(request, &state).await else {
        return error_response(V4Error::InvalidRequest, context.request_id());
    };
    let upload = match multipart::parse_worker_upload(multipart, api.bundle_limits).await {
        Ok(value) => value,
        Err(error) => return platform_error(context.request_id(), &error),
    };
    let now = match now_ms() {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    let _upload_guard = api.upload_serial.lock().await;
    let worker = match domain::worker_by_name(&api, account, &script) {
        Ok(worker) => Ok((worker, false)),
        Err(error) if deploy && error.code() == open_compute_core::ErrorCode::WorkerNotFound => {
            if let Err(error) = domain::validate_new_upload(
                &api,
                &account_authority,
                account,
                &script,
                &upload,
                query.strict_inheritance,
                now,
            )
            .await
            {
                return platform_error(context.request_id(), &error);
            }
            domain::ensure_worker(&api, account, &script, context.request_id(), now)
        }
        Err(error) => Err(error),
    };
    let (worker, created_worker) = match worker {
        Ok(value) => value,
        Err(error) => return platform_error(context.request_id(), &error),
    };
    let outcome = domain::create_from_upload(
        &api,
        &account_authority,
        account,
        &worker,
        upload,
        query.strict_inheritance,
        deploy.then_some(DeploymentSource::ScriptUpload),
        context.request_id(),
        now,
    )
    .await;
    if outcome.is_err() && created_worker {
        let repository = WorkerRepository::new(api.storage.db());
        let expected_versions = match repository.list_versions(account, worker.id) {
            Ok(versions) => versions
                .into_iter()
                .filter(|version| version.deleted_at_ms.is_none())
                .map(|version| version.id)
                .collect::<Vec<_>>(),
            Err(error) => return platform_error(context.request_id(), &error),
        };
        if let Err(cleanup) = repository.delete_worker(
            account,
            worker.id,
            &expected_versions,
            context.request_id(),
            now,
        ) {
            return platform_error(context.request_id(), &cleanup);
        }
    }
    match outcome {
        Ok(CreateVersionOutcome::Applied(result)) => {
            let snapshot = WorkerRepository::new(api.storage.db()).version_snapshot(
                account,
                worker.id,
                result.version.id,
                false,
            );
            match snapshot
                .map_err(|error| V4Error::from(&error))
                .and_then(|snapshot| {
                    VersionItem::from_snapshot(&api, &account_authority, &snapshot)
                }) {
                Ok(item) => success_response(context, item),
                Err(error) => error_response(error, context.request_id()),
            }
        }
        Ok(CreateVersionOutcome::Replay(bytes)) => {
            let version = serde_json::from_slice::<serde_json::Value>(&bytes)
                .ok()
                .and_then(|value| value["version"]["id"].as_str().map(str::to_owned))
                .and_then(|value| VersionId::from_str(&value).ok());
            let item = version
                .ok_or(V4Error::Internal)
                .and_then(|version| {
                    WorkerRepository::new(api.storage.db())
                        .version_snapshot(account, worker.id, version, false)
                        .map_err(|error| V4Error::from(&error))
                })
                .and_then(|snapshot| {
                    VersionItem::from_snapshot(&api, &account_authority, &snapshot)
                });
            match item {
                Ok(item) => success_response(context, item),
                Err(error) => error_response(error, context.request_id()),
            }
        }
        Err(error) => platform_error(context.request_id(), &error),
    }
}

async fn list_versions(
    State(state): State<HttpState>,
    Path((account, script)): Path<(String, String)>,
    request: Request,
) -> axum::response::Response {
    let context = match authorize(&request, V4Permission::Read) {
        Ok(value) => value,
        Err(response) => return response.into_response(),
    };
    let result = (|| {
        let query = query::version_list(request.uri().query())?;
        let account = domain::resolve_account(&state, &account)?;
        let api = worker_api(&state)?;
        let worker =
            domain::worker_by_name(api, account, &script).map_err(|error| V4Error::from(&error))?;
        let repo = WorkerRepository::new(api.storage.db());
        let mut records = repo
            .list_versions(account, worker.id)
            .map_err(|error| V4Error::from(&error))?;
        if query.deployable {
            records.retain(|version| version.state == open_compute_storage::VersionState::Ready);
        }
        let total = records.len();
        let start = if query.deployable {
            0
        } else {
            query.page.saturating_sub(1).saturating_mul(query.per_page)
        };
        let take = if query.deployable {
            total
        } else {
            query.per_page
        };
        let items = records
            .iter()
            .skip(start)
            .take(take)
            .map(|version| {
                let annotations = repo
                    .version_annotations(account, worker.id, version.id)
                    .map_err(|error| V4Error::from(&error))?;
                VersionShort::from_record(version, annotations)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let count = items.len();
        Ok((
            VersionList { items },
            V4ResultInfo {
                page: if query.deployable { 1 } else { query.page },
                per_page: take,
                count,
                total_count: total,
                total_pages: if query.deployable {
                    usize::from(total > 0)
                } else {
                    total.div_ceil(query.per_page)
                },
            },
        ))
    })();
    match result {
        Ok((result, info)) => paginated_response(context, result, info),
        Err(error) => error_response(error, context.request_id()),
    }
}

async fn get_version(
    State(state): State<HttpState>,
    Path((account, script, version)): Path<(String, String, String)>,
    request: Request,
) -> axum::response::Response {
    let context = match authorize(&request, V4Permission::Read) {
        Ok(value) => value,
        Err(response) => return response.into_response(),
    };
    let result = (|| {
        let account = domain::resolve_account(&state, &account)?;
        let api = worker_api(&state)?;
        let authority = state.cloudflare_v4_account().ok_or(V4Error::Unavailable)?;
        let worker =
            domain::worker_by_name(api, account, &script).map_err(|error| V4Error::from(&error))?;
        let version = VersionId::from_str(&version).map_err(|_| V4Error::InvalidRequest)?;
        let snapshot = WorkerRepository::new(api.storage.db())
            .version_snapshot(account, worker.id, version, false)
            .map_err(|error| V4Error::from(&error))?;
        VersionItem::from_snapshot(api, authority, &snapshot)
    })();
    respond(context, result)
}

async fn list_deployments(
    State(state): State<HttpState>,
    Path((account, script)): Path<(String, String)>,
    request: Request,
) -> axum::response::Response {
    let context = match authorize(&request, V4Permission::Read) {
        Ok(value) => value,
        Err(response) => return response.into_response(),
    };
    let result = (|| {
        let account = domain::resolve_account(&state, &account)?;
        let api = worker_api(&state)?;
        let worker =
            domain::worker_by_name(api, account, &script).map_err(|error| V4Error::from(&error))?;
        let deployments = WorkerRepository::new(api.storage.db())
            .list_deployments(account, worker.id)
            .map_err(|error| V4Error::from(&error))?
            .iter()
            .map(DeploymentItem::from_record)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(DeploymentList { deployments })
    })();
    respond(context, result)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateDeploymentBody {
    strategy: String,
    versions: Vec<CreateDeploymentVersion>,
    #[serde(default)]
    annotations: BTreeMap<String, String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateDeploymentVersion {
    version_id: VersionId,
    percentage: f64,
}

async fn create_deployment(
    State(state): State<HttpState>,
    Path((account, script)): Path<(String, String)>,
    request: Request,
) -> axum::response::Response {
    let context = match authorize(&request, V4Permission::ProductWrite) {
        Ok(value) => value,
        Err(response) => return response.into_response(),
    };
    match query::deployment_force(request.uri().query()) {
        Ok(false) => {}
        Ok(true) => return error_response(V4Error::Unsupported, context.request_id()),
        Err(error) => return error_response(error, context.request_id()),
    }
    let body = match json_body::<CreateDeploymentBody>(request).await {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    if body.strategy != "percentage"
        || body.versions.len() != 1
        || body.versions[0].percentage != 100.0
    {
        return error_response(V4Error::Unsupported, context.request_id());
    }
    if body.annotations.iter().any(|(key, value)| {
        key != "workers/message" || value.len() > 1_000 || value.chars().any(char::is_control)
    }) {
        return error_response(V4Error::InvalidRequest, context.request_id());
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
    let target = body.versions[0].version_id;
    let now = match now_ms() {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    let result = if let Some(promoter) = &api.product_promoter {
        promoter
            .promote(ProductPromotionRequest {
                account_id: account,
                worker_id: worker.id,
                version_id: target,
                source: DeploymentSource::VersionsApi,
                annotations: body.annotations.clone(),
                request_id: context.request_id(),
                now_ms: now,
            })
            .await
            .and_then(|_| {
                let refreshed = WorkerRepository::new(api.storage.db())
                    .get_tenant_worker(account, worker.id)?;
                WorkerRepository::new(api.storage.db()).get_deployment(
                    account,
                    worker.id,
                    refreshed.active_deployment_id.ok_or_else(|| {
                        PlatformError::new(
                            open_compute_core::ErrorCode::VersionInvariantViolation,
                            "active Deployment is missing",
                        )
                    })?,
                )
            })
    } else {
        WorkerRepository::new(api.storage.db())
            .create_deployment_checked(
                account,
                worker.id,
                target,
                None,
                Some(worker.route_generation),
                DeploymentSource::VersionsApi,
                &body.annotations,
                context.request_id(),
                now,
            )
            .map(|(_, deployment)| deployment)
    };
    match result {
        Ok(record) => match DeploymentItem::from_record(&record) {
            Ok(item) => success_response(context, item),
            Err(error) => error_response(error, context.request_id()),
        },
        Err(error) => platform_error(context.request_id(), &error),
    }
}

async fn get_deployment(
    State(state): State<HttpState>,
    Path((account, script, deployment)): Path<(String, String, String)>,
    request: Request,
) -> axum::response::Response {
    let context = match authorize(&request, V4Permission::Read) {
        Ok(value) => value,
        Err(response) => return response.into_response(),
    };
    let result = (|| {
        let account = domain::resolve_account(&state, &account)?;
        let api = worker_api(&state)?;
        let worker =
            domain::worker_by_name(api, account, &script).map_err(|error| V4Error::from(&error))?;
        let id = DeploymentId::from_str(&deployment).map_err(|_| V4Error::InvalidRequest)?;
        let record = WorkerRepository::new(api.storage.db())
            .get_deployment(account, worker.id, id)
            .map_err(|error| V4Error::from(&error))?;
        DeploymentItem::from_record(&record)
    })();
    respond(context, result)
}

async fn delete_deployment(
    State(state): State<HttpState>,
    Path((account, script, deployment)): Path<(String, String, String)>,
    request: Request,
) -> axum::response::Response {
    let context = match authorize(&request, V4Permission::ProductWrite) {
        Ok(value) => value,
        Err(response) => return response.into_response(),
    };
    let result = (|| {
        let account = domain::resolve_account(&state, &account)?;
        let api = worker_api(&state)?;
        let worker =
            domain::worker_by_name(api, account, &script).map_err(|error| V4Error::from(&error))?;
        let id = DeploymentId::from_str(&deployment).map_err(|_| V4Error::InvalidRequest)?;
        WorkerRepository::new(api.storage.db())
            .delete_deployment(account, worker.id, id, context.request_id(), now_ms()?)
            .map_err(|error| V4Error::from(&error))?;
        Ok(())
    })();
    respond(context, result)
}

pub(super) fn authorize(
    request: &Request,
    permission: V4Permission,
) -> Result<V4RequestContext, HttpError> {
    let context = request_context(request)?;
    context
        .require(permission)
        .map_err(|error| error_response(error, context.request_id()))?;
    Ok(context)
}

pub(super) fn worker_api(
    state: &HttpState,
) -> Result<&crate::workers_http::WorkerApiState, V4Error> {
    state
        .worker_api()
        .map(AsRef::as_ref)
        .ok_or(V4Error::Unavailable)
}

pub(super) fn respond<T: Serialize>(
    context: V4RequestContext,
    result: Result<T, V4Error>,
) -> axum::response::Response {
    match result {
        Ok(value) => success_response(context, value),
        Err(error) => error_response(error, context.request_id()),
    }
}

pub(super) fn platform_error(
    request_id: RequestId,
    error: &PlatformError,
) -> axum::response::Response {
    error_response(V4Error::from(error), request_id)
}

pub(super) fn timestamp(value: i64) -> Result<String, V4Error> {
    jiff::Timestamp::from_millisecond(value)
        .map(|timestamp| timestamp.to_string())
        .map_err(|_| V4Error::Internal)
}

pub(super) fn now_ms() -> Result<i64, V4Error> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| V4Error::Internal)
        .and_then(|duration| i64::try_from(duration.as_millis()).map_err(|_| V4Error::Internal))
}
