use super::*;
use axum::Json;
use axum::body::to_bytes;
use axum::response::IntoResponse as _;
use open_compute_storage::VersionState;

#[derive(Serialize)]
struct DeleteVersionResponse {
    errors: [serde_json::Value; 0],
    messages: [serde_json::Value; 0],
    success: bool,
}

pub(super) async fn get_beta_worker(
    State(state): State<HttpState>,
    Path((account, worker)): Path<(String, String)>,
    request: Request,
) -> axum::response::Response {
    let context = match authorize(&request, V4Permission::Read) {
        Ok(value) => value,
        Err(response) => return response.into_response(),
    };
    if request.uri().query().is_some() {
        return error_response(V4Error::InvalidRequest, context.request_id());
    }
    let account = match domain::resolve_instance(&state, &account) {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    let api = match worker_api(&state) {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    let Some(authority) = state.v4_instance_context() else {
        return error_response(V4Error::Unavailable, context.request_id());
    };
    let worker = match worker_by_identifier(
        WorkerRepository::new(api.storage.db()),
        authority,
        account,
        &worker,
    ) {
        Ok(value) => value,
        Err(error) => return platform_error(context.request_id(), &error),
    };
    let created_on = match crate::cloudflare_v4::iso_timestamp(worker.created_at_ms) {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    let updated_on = match crate::cloudflare_v4::iso_timestamp(worker.updated_at_ms) {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    success_response(
        context,
        serde_json::json!({
            "id": authority.public_worker_tag(worker.id),
            "name": worker.name,
            "tags": [],
            "subdomain": { "enabled": false, "previews_enabled": false },
            "observability": {},
            "logpush": false,
            "tail_consumers": [],
            "created_on": created_on,
            "updated_on": updated_on,
            "references": {
                "workers": [],
                "domains": [],
                "dispatch_namespace_outbounds": [],
                "durable_objects": [],
                "queues": [],
            },
        }),
    )
}

pub(super) async fn delete_beta_version(
    State(state): State<HttpState>,
    Path((account, worker, version)): Path<(String, String, String)>,
    request: Request,
) -> axum::response::Response {
    let context = match authorize(&request, V4Permission::ProductWrite) {
        Ok(value) => value,
        Err(response) => return response.into_response(),
    };
    if request.uri().query().is_some()
        || request
            .headers()
            .contains_key(axum::http::header::CONTENT_TYPE)
    {
        return error_response(V4Error::InvalidRequest, context.request_id());
    }
    match to_bytes(request.into_body(), 1).await {
        Ok(bytes) if bytes.is_empty() => {}
        _ => return error_response(V4Error::InvalidRequest, context.request_id()),
    }
    let account = match domain::resolve_instance(&state, &account) {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    let api = match worker_api(&state) {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    let Some(authority) = state.v4_instance_context() else {
        return error_response(V4Error::Unavailable, context.request_id());
    };
    let repository = WorkerRepository::new(api.storage.db());
    let worker = match worker_by_identifier(repository, authority, account, &worker) {
        Ok(value) => value,
        Err(error) => return platform_error(context.request_id(), &error),
    };
    let version = match resolve_version(repository, account, worker.id, &version) {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    if version.state == VersionState::Tombstoned {
        return deleted();
    }
    let storage = api.storage.clone();
    let worker_id = worker.id;
    let version_id = version.id;
    let begin = tokio::task::spawn_blocking(move || {
        WorkerRepository::new(storage.db()).begin_version_delete(account, worker_id, version_id)
    })
    .await;
    match begin {
        Ok(Ok(())) => {}
        Ok(Err(error)) => {
            return platform_error(context.request_id(), &error);
        }
        Err(_) => return error_response(V4Error::Internal, context.request_id()),
    }
    if let Err(error) = api
        .pins
        .fence_and_wait(version_id, api.delete_drain_timeout)
        .await
    {
        return platform_error(context.request_id(), &error);
    }
    let storage = api.storage.clone();
    let finish = tokio::task::spawn_blocking(move || {
        WorkerRepository::new(storage.db()).finalize_version_delete(
            account,
            worker_id,
            version_id,
            context.request_id(),
            now_ms(),
        )
    })
    .await;
    match finish {
        Ok(Ok(())) => {
            api.pins.retire_fence(version_id);
            deleted()
        }
        Ok(Err(error)) => platform_error(context.request_id(), &error),
        Err(_) => error_response(V4Error::Internal, context.request_id()),
    }
}

fn worker_by_identifier(
    repository: WorkerRepository<'_>,
    authority: &crate::cloudflare_v4::accounts::V4InstanceContext,
    account: open_compute_core::InstanceId,
    requested: &str,
) -> Result<WorkerRecord, PlatformError> {
    repository.list_workers(account).and_then(|workers| {
        workers
            .into_iter()
            .find(|worker| {
                worker.name == requested
                    || authority.matches_public_worker_tag(worker.id, requested)
            })
            .ok_or_else(|| {
                PlatformError::new(
                    open_compute_core::ErrorCode::WorkerNotFound,
                    "Worker was not found",
                )
            })
    })
}

fn resolve_version(
    repository: WorkerRepository<'_>,
    account: open_compute_core::InstanceId,
    worker: open_compute_core::WorkerId,
    requested: &str,
) -> Result<VersionRecord, V4Error> {
    let versions = repository
        .list_versions(account, worker)
        .map_err(|error| V4Error::from(&error))?;
    if requested == "latest" {
        return versions.into_iter().next().ok_or(V4Error::NotFound);
    }
    if let Ok(id) = VersionId::from_str(requested) {
        return versions
            .into_iter()
            .find(|version| version.id == id)
            .ok_or(V4Error::NotFound);
    }
    if requested.len() < 8
        || !requested
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() || byte == b'-')
    {
        return Err(V4Error::InvalidRequest);
    }
    let mut matches = versions
        .into_iter()
        .filter(|version| version.id.to_string().starts_with(requested));
    let version = matches.next().ok_or(V4Error::NotFound)?;
    if matches.next().is_some() {
        return Err(V4Error::Conflict);
    }
    Ok(version)
}

fn deleted() -> axum::response::Response {
    Json(DeleteVersionResponse {
        errors: [],
        messages: [],
        success: true,
    })
    .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode, header};
    use open_compute_core::{RequestId, SecretString};
    use open_compute_storage::{
        NewVersion, NewVersionProducts, VersionContentKind, WorkerRepository,
    };
    use std::collections::BTreeMap;
    use tower::ServiceExt as _;

    fn ready_version(
        repository: WorkerRepository<'_>,
        account: open_compute_core::InstanceId,
        worker: open_compute_core::WorkerId,
        now: i64,
    ) -> VersionId {
        let id = VersionId::generate();
        repository
            .insert_staging_version(
                &NewVersion {
                    id,
                    instance_id: account,
                    worker_id: worker,
                    content_kind: VersionContentKind::Worker,
                    artifact_sha256: Some([1; 32]),
                    artifact_size: Some(1),
                    artifact_schema_version: Some(1),
                    main_module: Some("index.js".to_owned()),
                    worker_code_sha256: [2; 32],
                    compatibility_date: "2026-09-08".to_owned(),
                    compatibility_flags: Vec::new(),
                    resource_limits:
                        open_compute_storage::EffectiveResourceLimits::standard_defaults(),
                    vars: BTreeMap::new(),
                    secrets: BTreeMap::new(),
                    request_id: RequestId::generate(),
                    now_ms: now,
                },
                &NewVersionProducts::default(),
                100,
            )
            .unwrap();
        repository.begin_validation(id).unwrap();
        repository.mark_ready(id, now + 1).unwrap();
        id
    }

    #[tokio::test]
    async fn beta_delete_tombstones_only_non_active_versions_and_replays() {
        let (_temp, _mock, state, account, storage) =
            crate::tests::initialized_worker_http_fixture().await;
        let repository = WorkerRepository::new(storage.db());
        let worker = repository
            .create_worker(account, "versions", RequestId::generate(), 1, 100)
            .unwrap()
            .0;
        let historical = ready_version(repository, account, worker.id, 2);
        let active = ready_version(repository, account, worker.id, 4);
        repository
            .promote(account, worker.id, active, None, RequestId::generate(), 6)
            .unwrap();
        let authority = crate::cloudflare_v4::accounts::V4InstanceContext::new(account, 1);
        let worker_tag = authority.public_worker_tag(worker.id);
        let worker_path = format!(
            "/client/v4/accounts/{}/workers/workers/versions",
            authority.public_id()
        );
        let prefix = format!(
            "/client/v4/accounts/{}/workers/workers/versions/versions/",
            authority.public_id()
        );
        let app = crate::http::admin_router(
            state
                .with_platform_storage(storage.clone())
                .with_v4_tokens(
                    SecretString::new("deployer-token"),
                    SecretString::new("read-token"),
                )
                .with_v4_instance_context(authority),
        );
        assert_eq!(
            app.clone()
                .oneshot(
                    Request::builder()
                        .uri(&worker_path)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            app.clone()
                .oneshot(
                    Request::builder()
                        .uri(format!("{worker_path}?unexpected=true"))
                        .header(header::AUTHORIZATION, "Bearer read-token")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap()
                .status(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            app.clone()
                .oneshot(
                    Request::builder()
                        .uri(worker_path.replace("/versions", "/missing"))
                        .header(header::AUTHORIZATION, "Bearer read-token")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap()
                .status(),
            StatusCode::NOT_FOUND
        );
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(worker_path)
                    .header(header::AUTHORIZATION, "Bearer read-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body: serde_json::Value =
            serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        assert_eq!(body["result"]["id"], worker_tag);
        assert_eq!(body["result"]["subdomain"]["enabled"], false);
        let send = |version: VersionId| {
            Request::builder()
                .method("DELETE")
                .uri(format!("{prefix}{version}"))
                .header(header::AUTHORIZATION, "Bearer deployer-token")
                .body(Body::empty())
                .unwrap()
        };
        for request in [
            Request::builder()
                .method("DELETE")
                .uri(format!("{prefix}{historical}"))
                .body(Body::empty())
                .unwrap(),
            Request::builder()
                .method("DELETE")
                .uri(format!("{prefix}{historical}?unexpected=true"))
                .header(header::AUTHORIZATION, "Bearer deployer-token")
                .body(Body::empty())
                .unwrap(),
            Request::builder()
                .method("DELETE")
                .uri(format!("{prefix}{historical}"))
                .header(header::AUTHORIZATION, "Bearer deployer-token")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::empty())
                .unwrap(),
            Request::builder()
                .method("DELETE")
                .uri(format!("{prefix}{historical}"))
                .header(header::AUTHORIZATION, "Bearer deployer-token")
                .body(Body::from("x"))
                .unwrap(),
        ] {
            assert!(
                app.clone()
                    .oneshot(request)
                    .await
                    .unwrap()
                    .status()
                    .is_client_error()
            );
        }
        for requested in ["bad", "deadbeef"] {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method("DELETE")
                        .uri(format!("{prefix}{requested}"))
                        .header(header::AUTHORIZATION, "Bearer deployer-token")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert!(response.status().is_client_error());
        }
        assert_eq!(
            app.clone().oneshot(send(active)).await.unwrap().status(),
            StatusCode::CONFLICT
        );
        assert_eq!(
            app.clone()
                .oneshot(send(historical))
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );
        assert_eq!(
            app.oneshot(send(historical)).await.unwrap().status(),
            StatusCode::OK
        );
        assert_eq!(
            repository
                .list_versions(account, worker.id)
                .unwrap()
                .into_iter()
                .find(|version| version.id == historical)
                .unwrap()
                .state,
            VersionState::Tombstoned
        );
        assert_eq!(
            resolve_version(repository, account, worker.id, "latest")
                .unwrap()
                .id,
            active
        );
        let historical_text = historical.to_string();
        assert_eq!(
            resolve_version(repository, account, worker.id, &historical_text[..8]).unwrap_err(),
            V4Error::Conflict
        );
        assert_eq!(
            resolve_version(repository, account, worker.id, &historical_text[..24])
                .unwrap()
                .id,
            historical
        );
    }
}
