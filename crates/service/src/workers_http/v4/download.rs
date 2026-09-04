//! Raw Worker Script download reconstruction from immutable Version artifacts.

use super::{domain, handlers::platform_error};
use crate::cloudflare_v4::{V4Error, V4RequestContext, error_response};
use crate::http::{HttpState, REQUEST_ID_HEADER};
use axum::body::Body;
use axum::extract::Request;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::Response;
use open_compute_artifacts::{ARTIFACT_KEY_VERSION, ArtifactRef};
use open_compute_storage::WorkerRepository;
use open_compute_workers::{CanonicalBundle, ModuleType};

pub(super) async fn download_script(
    state: HttpState,
    account: String,
    script: String,
    request: Request,
    context: V4RequestContext,
) -> Response {
    let account = match domain::resolve_account(&state, &account) {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    let Some(api) = state.worker_api() else {
        return error_response(V4Error::Unavailable, context.request_id());
    };
    let worker = match domain::worker_by_name(api, account, &script) {
        Ok(value) => value,
        Err(error) => return platform_error(context.request_id(), &error),
    };
    let Some(version_id) = worker.active_version_id else {
        return error_response(V4Error::NotFound, context.request_id());
    };
    let snapshot = match WorkerRepository::new(api.storage.db())
        .version_snapshot(account, worker.id, version_id, false)
    {
        Ok(value) => value,
        Err(error) => return platform_error(context.request_id(), &error),
    };
    let etag = format!("\"{}\"", hex::encode(snapshot.version.worker_code_sha256));
    if request
        .headers()
        .get(header::IF_NONE_MATCH)
        .and_then(|value| value.to_str().ok())
        == Some(etag.as_str())
    {
        return raw_response(context, StatusCode::NOT_MODIFIED, None, &etag, Vec::new());
    }
    if snapshot.version.content_kind == open_compute_storage::VersionContentKind::AssetsOnly {
        return multipart_response(context, &etag, None, "assets-only");
    }
    let Some(digest) = snapshot.version.artifact_sha256 else {
        return error_response(V4Error::Internal, context.request_id());
    };
    let Some(size) = snapshot.version.artifact_size else {
        return error_response(V4Error::Internal, context.request_id());
    };
    let artifact = match ArtifactRef::new(ARTIFACT_KEY_VERSION, &hex::encode(digest), size) {
        Ok(value) => value,
        Err(error) => return platform_error(context.request_id(), &error),
    };
    let bytes = match api.artifacts.open(&artifact).await {
        Ok(value) => value,
        Err(error) => return platform_error(context.request_id(), &error),
    };
    let bundle = match CanonicalBundle::parse(bytes.to_vec(), api.bundle_limits) {
        Ok(value) => value,
        Err(error) => return platform_error(context.request_id(), &error),
    };
    let Some(main) = bundle
        .manifest()
        .modules
        .iter()
        .find(|module| module.name == bundle.manifest().main_module)
    else {
        return error_response(V4Error::Internal, context.request_id());
    };
    if main.module_type == ModuleType::CommonJsModule {
        let body = match bundle.module_bytes(main) {
            Ok(value) => value.to_vec(),
            Err(error) => return platform_error(context.request_id(), &error),
        };
        return raw_response(
            context,
            StatusCode::OK,
            Some("application/javascript"),
            &etag,
            body,
        );
    }
    multipart_response(context, &etag, Some(&bundle), &hex::encode(digest)[..16])
}

fn multipart_response(
    context: V4RequestContext,
    etag: &str,
    bundle: Option<&CanonicalBundle>,
    boundary_suffix: &str,
) -> Response {
    let boundary = format!("open-compute-{boundary_suffix}");
    let mut body = Vec::new();
    for module in bundle
        .into_iter()
        .flat_map(|value| &value.manifest().modules)
    {
        body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        body.extend_from_slice(
            format!(
                "Content-Disposition: form-data; name=\"{}\"; filename=\"{}\"\r\n",
                module.name, module.name
            )
            .as_bytes(),
        );
        body.extend_from_slice(
            format!(
                "Content-Type: {}\r\n\r\n",
                module_content_type(module.module_type)
            )
            .as_bytes(),
        );
        let Some(bytes) = bundle.and_then(|value| value.module_bytes(module).ok()) else {
            return error_response(V4Error::Internal, context.request_id());
        };
        body.extend_from_slice(bytes);
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    raw_response(
        context,
        StatusCode::OK,
        Some(&format!("multipart/form-data; boundary={boundary}")),
        etag,
        body,
    )
}

fn module_content_type(module_type: ModuleType) -> &'static str {
    match module_type {
        ModuleType::EsModule => "application/javascript+module",
        ModuleType::CommonJsModule => "application/javascript",
        ModuleType::Text => "text/plain",
        ModuleType::Data => "application/octet-stream",
        ModuleType::Json => "application/json",
        ModuleType::Wasm => "application/wasm",
        ModuleType::SourceMap => "application/source-map",
    }
}

fn raw_response(
    context: V4RequestContext,
    status: StatusCode,
    content_type: Option<&str>,
    etag: &str,
    body: Vec<u8>,
) -> Response {
    let mut response = Response::new(Body::from(body));
    *response.status_mut() = status;
    for (name, value) in [
        (REQUEST_ID_HEADER, context.request_id().to_string()),
        (header::ETAG.as_str(), etag.to_owned()),
    ] {
        if let Ok(value) = HeaderValue::from_str(&value) {
            response
                .headers_mut()
                .insert(axum::http::HeaderName::from_static(name), value);
        }
    }
    if let Some(content_type) = content_type
        && let Ok(value) = HeaderValue::from_str(content_type)
    {
        response.headers_mut().insert(header::CONTENT_TYPE, value);
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cloudflare_v4::accounts::AccountAuthority;
    use crate::http;
    use axum::body::to_bytes;
    use open_compute_core::{PlatformId, RequestId, SecretString};
    use open_compute_storage::{DeploymentSource, WorkerRepository};
    use open_compute_workers::{
        AssetEntryV1, AssetManifestV1, AssetRoutingConfigV1, CreateVersionOutcome,
        CreateVersionRequest, ModuleInput, NotFoundHandling, RunWorkerFirst, RuntimeValidator,
        ValidationCandidate, VersionAssets, VersionBundle, VersionContent, VersionController,
    };
    use sha2::{Digest as _, Sha256};
    use std::collections::BTreeMap;
    use std::sync::Arc;
    use tower::ServiceExt as _;

    async fn seed_worker(
        api: &crate::workers_http::WorkerApiState,
        account: open_compute_core::AccountId,
        name: &str,
        modules: Vec<ModuleInput>,
    ) {
        let worker = WorkerRepository::new(api.storage.db())
            .create_worker(account, name, RequestId::generate(), 1, 100)
            .unwrap()
            .0;
        let main = modules[0].name.clone();
        let bundle = CanonicalBundle::build(&main, modules, api.bundle_limits).unwrap();
        let validator: Arc<dyn RuntimeValidator> =
            Arc::new(|_: ValidationCandidate| async { Ok(()) });
        let result = VersionController::new(
            &api.storage,
            api.artifacts.clone(),
            validator,
            api.bundle_limits,
        )
        .create_version(CreateVersionRequest {
            account_id: account,
            worker_id: worker.id,
            idempotency_key: format!("seed-{name}"),
            content: VersionContent::Worker {
                bundle: VersionBundle::Bytes(bundle.into_bytes()),
                assets: None,
            },
            vars: BTreeMap::new(),
            secrets: BTreeMap::new(),
            bindings: BTreeMap::new(),
            services: BTreeMap::new(),
            runtime_features: Default::default(),
            queue_consumers: Vec::new(),
            crons: Vec::new(),
            deployment_source: Some(DeploymentSource::ScriptUpload),
            request_id: RequestId::generate(),
            now_ms: 10,
        })
        .await
        .unwrap();
        assert!(matches!(result, CreateVersionOutcome::Applied(_)));
    }

    async fn seed_assets_only(
        api: &crate::workers_http::WorkerApiState,
        account: open_compute_core::AccountId,
    ) {
        let worker = WorkerRepository::new(api.storage.db())
            .create_worker(account, "assets-only", RequestId::generate(), 1, 100)
            .unwrap()
            .0;
        let bytes = b"asset".to_vec();
        let digest: [u8; 32] = Sha256::digest(&bytes).into();
        api.artifacts
            .put_verified(
                futures::stream::once(async move {
                    Ok::<bytes::Bytes, std::io::Error>(bytes::Bytes::from(bytes))
                }),
                &hex::encode(digest),
                5,
            )
            .await
            .unwrap();
        let validator: Arc<dyn RuntimeValidator> =
            Arc::new(|_: ValidationCandidate| async { Ok(()) });
        VersionController::new(
            &api.storage,
            api.artifacts.clone(),
            validator,
            api.bundle_limits,
        )
        .create_version(CreateVersionRequest {
            account_id: account,
            worker_id: worker.id,
            idempotency_key: "seed-assets-only".to_owned(),
            content: VersionContent::AssetsOnly {
                assets: VersionAssets {
                    manifest: AssetManifestV1 {
                        schema_version: 1,
                        entries: vec![AssetEntryV1 {
                            path: "/asset.txt".to_owned(),
                            sha256: hex::encode(digest),
                            size: 5,
                            content_type: "text/plain".to_owned(),
                        }],
                    },
                    routing: AssetRoutingConfigV1 {
                        schema_version: 1,
                        binding: None,
                        run_worker_first: RunWorkerFirst::All(false),
                        html_handling: Default::default(),
                        not_found_handling: NotFoundHandling::None,
                        headers: Vec::new(),
                        redirects: Vec::new(),
                    },
                },
            },
            vars: BTreeMap::new(),
            secrets: BTreeMap::new(),
            bindings: BTreeMap::new(),
            services: BTreeMap::new(),
            runtime_features: Default::default(),
            queue_consumers: Vec::new(),
            crons: Vec::new(),
            deployment_source: Some(DeploymentSource::ScriptUpload),
            request_id: RequestId::generate(),
            now_ms: 20,
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn reconstructs_service_worker_modules_etag_and_assets_only_downloads() {
        let (_dir, _mock, state, account, _storage) =
            crate::tests::initialized_worker_http_fixture().await;
        let api = state.worker_api().unwrap().clone();
        seed_worker(
            &api,
            account,
            "service-worker",
            vec![ModuleInput {
                name: "index.js".to_owned(),
                module_type: ModuleType::CommonJsModule,
                bytes: b"addEventListener('fetch',()=>{});".to_vec(),
            }],
        )
        .await;
        seed_worker(
            &api,
            account,
            "module-worker",
            vec![
                ModuleInput {
                    name: "index.js".to_owned(),
                    module_type: ModuleType::EsModule,
                    bytes: b"export default {};".to_vec(),
                },
                ModuleInput {
                    name: "index.js.map".to_owned(),
                    module_type: ModuleType::SourceMap,
                    bytes: br#"{"version":3}"#.to_vec(),
                },
            ],
        )
        .await;
        seed_assets_only(&api, account).await;
        let authority = AccountAuthority::new(PlatformId::generate(), account, 1_000);
        let public_account = authority.public_id().to_owned();
        let app = http::admin_router(
            state
                .with_v4_tokens(
                    SecretString::new("deployer-token"),
                    SecretString::new("read-token"),
                )
                .with_cloudflare_v4_account(authority),
        );
        let get = |name: &str| {
            Request::builder()
                .uri(format!(
                    "/client/v4/accounts/{public_account}/workers/scripts/{name}"
                ))
                .header("authorization", "Bearer read-token")
                .header("accept", "*/*")
                .body(Body::empty())
                .unwrap()
        };
        let service = app.clone().oneshot(get("service-worker")).await.unwrap();
        assert_eq!(service.status(), StatusCode::OK);
        assert_eq!(
            service.headers()[header::CONTENT_TYPE],
            "application/javascript"
        );
        let etag = service.headers()[header::ETAG].to_str().unwrap().to_owned();
        assert_eq!(
            to_bytes(service.into_body(), 64 * 1024).await.unwrap(),
            "addEventListener('fetch',()=>{});"
        );
        let not_modified = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/client/v4/accounts/{public_account}/workers/scripts/service-worker"
                    ))
                    .header("authorization", "Bearer read-token")
                    .header(header::IF_NONE_MATCH, etag)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(not_modified.status(), StatusCode::NOT_MODIFIED);
        assert!(
            to_bytes(not_modified.into_body(), 1)
                .await
                .unwrap()
                .is_empty()
        );

        let modules = app.clone().oneshot(get("module-worker")).await.unwrap();
        assert!(
            modules.headers()[header::CONTENT_TYPE]
                .to_str()
                .unwrap()
                .starts_with("multipart/form-data; boundary=")
        );
        let modules_body = to_bytes(modules.into_body(), 64 * 1024).await.unwrap();
        assert!(
            modules_body
                .windows(22)
                .any(|value| value == b"application/source-map")
        );
        assert!(!modules_body.windows(8).any(|value| value == b"metadata"));

        let assets = app.oneshot(get("assets-only")).await.unwrap();
        assert_eq!(assets.status(), StatusCode::OK);
        assert_eq!(
            to_bytes(assets.into_body(), 1024).await.unwrap(),
            "--open-compute-assets-only--\r\n"
        );
    }
}
