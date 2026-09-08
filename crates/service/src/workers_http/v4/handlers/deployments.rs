use super::*;

pub(super) async fn list_versions(
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

pub(super) async fn get_version(
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

pub(super) async fn list_deployments(
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

pub(super) async fn create_deployment(
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
    let now = now_ms();
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

pub(super) async fn get_deployment(
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

pub(super) async fn delete_deployment(
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
            .delete_deployment(account, worker.id, id, context.request_id(), now_ms())
            .map_err(|error| V4Error::from(&error))?;
        Ok(())
    })();
    respond(context, result)
}
