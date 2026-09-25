use super::*;

pub(crate) async fn delete_script(
    State(state): State<HttpState>,
    Path((account, script)): Path<(String, String)>,
    request: Request,
) -> Response {
    let context = match authorize(&request, V4Permission::ProductWrite) {
        Ok(value) => value,
        Err(response) => return response.into_response(),
    };
    let force = match delete_force_query(request.uri().query()) {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    let account = match domain::resolve_instance(&state, &account) {
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
    if force
        && let Err(error) = repo.begin_force_delete(account, worker.id, context.request_id(), now)
    {
        return platform_error(context.request_id(), &error);
    }
    let versions = match repo.list_versions(account, worker.id) {
        Ok(values) => values
            .into_iter()
            .filter(|version| version.deleted_at_ms.is_none())
            .map(|version| version.id)
            .collect::<Vec<_>>(),
        Err(error) => return platform_error(context.request_id(), &error),
    };
    let loader_prefix = open_compute_workers::worker_loader_namespace_prefix(account, worker.id);
    let drained = api
        .pins
        .fence_many_and_wait(&versions, api.delete_drain_timeout)
        .await;
    if drained.is_err() && force {
        if let Err(error) = api
            .transport
            .rotate_generation(api.delete_drain_timeout)
            .await
        {
            return platform_error(context.request_id(), &error);
        }
        if let Err(error) = api
            .pins
            .fence_many_and_wait(&versions, api.delete_drain_timeout)
            .await
        {
            return platform_error(context.request_id(), &error);
        }
    } else if let Err(error) = drained {
        for version in &versions {
            api.pins.unfence(*version);
        }
        return platform_error(context.request_id(), &error);
    }
    if let Some(cache) = &api.response_cache
        && let Err(error) = cache.purge_worker(account, worker.id, now)
    {
        if !force {
            for version in &versions {
                api.pins.unfence(*version);
            }
        }
        return platform_error(context.request_id(), &error);
    }
    let deletion = if force {
        repo.finish_force_delete(account, worker.id, &versions, context.request_id(), now)
    } else {
        repo.delete_worker(account, worker.id, &versions, context.request_id(), now)
    };
    if let Err(error) = deletion {
        if !force {
            for version in &versions {
                api.pins.unfence(*version);
            }
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
    if let Some(generation) = api.transport.current_generation()
        && let Err(error) = api
            .transport
            .revoke_worker_loader_prefix(loader_prefix, generation)
            .await
    {
        return platform_error(context.request_id(), &error);
    }
    success_response(context, ())
}

pub(super) fn delete_force_query(query: Option<&str>) -> Result<bool, V4Error> {
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
