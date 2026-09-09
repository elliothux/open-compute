use super::*;

pub(super) async fn list_scripts(
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

pub(super) async fn get_script(
    State(state): State<HttpState>,
    Path((account, script)): Path<(String, String)>,
    request: Request,
) -> axum::response::Response {
    let context = match authorize(&request, V4Permission::Read) {
        Ok(value) => value,
        Err(response) => return response.into_response(),
    };
    super::super::download::download_script(state, account, script, request, context).await
}

pub(super) async fn put_script(
    State(state): State<HttpState>,
    Path((account, script)): Path<(String, String)>,
    request: Request,
) -> axum::response::Response {
    upload(state, account, script, request, true).await
}

pub(super) async fn post_version(
    State(state): State<HttpState>,
    Path((account, script)): Path<(String, String)>,
    request: Request,
) -> axum::response::Response {
    upload(state, account, script, request, false).await
}

pub(super) async fn upload(
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
    let request = match super::super::sdk_multipart::normalize_request(request).await {
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
    let upload_observability = upload.metadata.observability.clone();
    let now = now_ms();
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
    if outcome.is_ok()
        && let Some(observability) = &upload_observability
    {
        let repository = WorkerRepository::new(api.storage.db());
        if let Err(error) = apply_upload_observability(
            repository,
            account,
            worker.id,
            observability,
            context.request_id(),
            now,
        ) {
            return platform_error(context.request_id(), &error);
        }
    }
    match outcome {
        Ok(CreateVersionOutcome::Applied(result)) => {
            let repository = WorkerRepository::new(api.storage.db());
            let snapshot =
                repository.version_snapshot(account, worker.id, result.version.id, false);
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

pub(super) fn apply_upload_observability(
    repository: WorkerRepository<'_>,
    account: open_compute_core::AccountId,
    worker: open_compute_core::WorkerId,
    value: &super::super::model::WorkerUploadObservability,
    request_id: RequestId,
    now_ms: i64,
) -> Result<(), PlatformError> {
    let current = repository.get_observability_settings(account, worker)?;
    let logs = value.logs.as_ref();
    let replacement = UpdateWorkerObservabilitySettings {
        enabled: value.enabled,
        head_sampling_rate: value.head_sampling_rate.or(current.head_sampling_rate),
        logs_enabled: logs
            .and_then(|settings| settings.enabled)
            .unwrap_or(current.logs_enabled),
        logs_head_sampling_rate: logs
            .and_then(|settings| settings.head_sampling_rate)
            .or(current.logs_head_sampling_rate),
        invocation_logs: logs
            .and_then(|settings| settings.invocation_logs)
            .unwrap_or(current.invocation_logs),
        persist: logs
            .and_then(|settings| settings.persist)
            .unwrap_or(current.persist),
    };
    if same_observability(&current, &replacement) {
        return Ok(());
    }
    repository.update_observability_settings(account, worker, &replacement, request_id, now_ms)?;
    Ok(())
}

pub(super) fn same_observability(
    current: &open_compute_storage::WorkerObservabilitySettings,
    replacement: &UpdateWorkerObservabilitySettings,
) -> bool {
    current.enabled == replacement.enabled
        && current.head_sampling_rate == replacement.head_sampling_rate
        && current.logs_enabled == replacement.logs_enabled
        && current.logs_head_sampling_rate == replacement.logs_head_sampling_rate
        && current.invocation_logs == replacement.invocation_logs
        && current.persist == replacement.persist
}
