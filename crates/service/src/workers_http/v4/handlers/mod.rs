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
    DeploymentRecord, DeploymentSource, UpdateWorkerObservabilitySettings, VersionRecord,
    VersionSnapshot, WorkerRecord, WorkerRepository,
};
use open_compute_workers::{CreateVersionOutcome, ProductPromotionRequest};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::str::FromStr;

pub(super) use super::json::json_body;

/// Compose the fixed Wrangler Worker-management subset.
pub(crate) fn router() -> Router<HttpState> {
    Router::new()
        .merge(super::observability::router())
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
    #[serde(skip_serializing_if = "Option::is_none")]
    migration_tag: Option<String>,
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
                    migration_tag: open_compute_storage::DurableObjectRepository::new(&api.storage)
                        .current_worker_migration(worker.id)
                        .map_err(|error| V4Error::from(&error))?
                        .map(|head| head.tag),
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
            created_on: crate::cloudflare_v4::iso_timestamp(worker.created_at_ms)?,
            modified_on: crate::cloudflare_v4::iso_timestamp(worker.updated_at_ms)?,
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
        let created = crate::cloudflare_v4::iso_timestamp(version.created_at_ms)?;
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
        let created = crate::cloudflare_v4::iso_timestamp(version.created_at_ms)?;
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
            created_on: crate::cloudflare_v4::iso_timestamp(record.created_at_ms)?,
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

mod deployments;
mod scripts;

use deployments::*;
use scripts::*;

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

pub(super) fn now_ms() -> i64 {
    open_compute_core::wall_time_ms()
}
