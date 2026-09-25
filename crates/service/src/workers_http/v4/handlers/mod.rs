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
    DeploymentRecord, DeploymentSource, QueueConsumerRepository, QueueRepository, VersionRecord,
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
            "/accounts/{account}/workers/scripts/{script}/queue-consumers",
            get(list_queue_consumers),
        )
        .route(
            "/accounts/{account}/workers/scripts/{script}/versions/{version}",
            get(get_version),
        )
        .route(
            "/accounts/{account}/workers/workers/{worker}",
            get(get_beta_worker),
        )
        .route(
            "/accounts/{account}/workers/workers/{worker}/versions/{version}",
            axum::routing::delete(delete_beta_version),
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

async fn list_queue_consumers(
    State(state): State<HttpState>,
    Path((account, script)): Path<(String, String)>,
    request: Request,
) -> axum::response::Response {
    let context = match authorize(&request, V4Permission::Read) {
        Ok(value) => value,
        Err(response) => return response.into_response(),
    };
    let query = match query::queue_consumers(request.uri().query()) {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    let result = (|| {
        let account_id = domain::resolve_instance(&state, &account)?;
        let authority = state.v4_instance_context().ok_or(V4Error::Unavailable)?;
        let api = worker_api(&state)?;
        let worker = domain::worker_by_name(api, account_id, &script)
            .map_err(|error| V4Error::from(&error))?;
        let consumers = QueueConsumerRepository::new(api.storage.db())
            .live_for_worker(worker.id)
            .map_err(|error| V4Error::from(&error))?;
        let total = consumers.len();
        let start = query.page.saturating_sub(1).saturating_mul(query.per_page);
        let page = consumers
            .into_iter()
            .skip(start)
            .take(query.per_page)
            .map(|record| {
                let queue = QueueRepository::new(api.storage.db())
                    .get(account_id, record.queue_id)
                    .map_err(|error| V4Error::from(&error))?;
                crate::cloudflare_v4::queues::consumers::consumer_response(
                    authority,
                    &api.storage,
                    &queue,
                    &record,
                )
                .map_err(|error| V4Error::from(&error))
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok((page, total))
    })();
    match result {
        Ok((page, total)) => {
            let count = page.len();
            paginated_response(
                context,
                page,
                V4ResultInfo {
                    page: query.page,
                    per_page: query.per_page,
                    count,
                    total_count: total,
                    total_pages: total.div_ceil(query.per_page),
                },
            )
        }
        Err(error) => error_response(error, context.request_id()),
    }
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
    #[serde(skip_serializing_if = "Option::is_none")]
    limits: Option<ServiceLimits>,
}

#[derive(Serialize)]
struct ServiceLimits {
    cpu_ms: u32,
    subrequests: u32,
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
        let account = domain::resolve_instance(&state, &account)?;
        let authority = state.v4_instance_context().ok_or(V4Error::Unavailable)?;
        let api = worker_api(&state)?;
        let worker =
            domain::worker_by_name(api, account, &script).map_err(|error| V4Error::from(&error))?;
        let limits = worker
            .active_version_id
            .map(|version| {
                WorkerRepository::new(api.storage.db())
                    .get_version(account, worker.id, version)
                    .map(|version| ServiceLimits {
                        cpu_ms: version.resource_limits.cpu_ms,
                        subrequests: version.resource_limits.sub_requests,
                    })
                    .map_err(|error| V4Error::from(&error))
            })
            .transpose()?;
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
                    limits,
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
    limits: VersionCpuLimits,
    usage_model: &'static str,
}

#[derive(Serialize)]
struct VersionCpuLimits {
    cpu_ms: u32,
}

impl VersionItem {
    fn from_snapshot(
        api: &crate::workers_http::WorkerApiState,
        authority: &crate::cloudflare_v4::accounts::V4InstanceContext,
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
                    limits: VersionCpuLimits {
                        cpu_ms: version.resource_limits.cpu_ms,
                    },
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
mod versions;

use deployments::*;
use scripts::*;
use versions::*;

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
    tracing::error!(
        request_id = %request_id,
        platform_error_code = error.code().as_str(),
        platform_error_message = error.message(),
        "Worker management request failed"
    );
    error_response(V4Error::from(error), request_id)
}

pub(super) fn now_ms() -> i64 {
    open_compute_core::wall_time_ms()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;
    use open_compute_core::ErrorCode;
    use std::io::Write;
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Default)]
    struct LogBuffer(Arc<Mutex<Vec<u8>>>);

    struct LogWriter(Arc<Mutex<Vec<u8>>>);

    impl Write for LogWriter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogBuffer {
        type Writer = LogWriter;

        fn make_writer(&'a self) -> Self::Writer {
            LogWriter(self.0.clone())
        }
    }

    #[tokio::test]
    async fn platform_errors_log_stable_operator_cause_without_changing_wire_error() {
        let buffer = LogBuffer::default();
        let subscriber = tracing_subscriber::fmt()
            .json()
            .without_time()
            .with_writer(buffer.clone())
            .finish();
        let request_id = RequestId::generate();
        let error = PlatformError::new(ErrorCode::BundleInvalid, "safe operator cause");
        let response =
            tracing::subscriber::with_default(subscriber, || platform_error(request_id, &error));
        assert_eq!(response.status(), axum::http::StatusCode::BAD_REQUEST);
        let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        let body = String::from_utf8(body.to_vec()).unwrap();
        assert!(body.contains("9100003"));
        assert!(!body.contains("safe operator cause"));

        let log = String::from_utf8(buffer.0.lock().unwrap().clone()).unwrap();
        assert!(log.contains(&request_id.to_string()));
        assert!(log.contains(ErrorCode::BundleInvalid.as_str()));
        assert!(log.contains("safe operator cause"));
    }
}
