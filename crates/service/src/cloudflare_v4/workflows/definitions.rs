//! Workflow definition and immutable version routes.

use super::*;
use crate::cloudflare_v4::storage::{iso_timestamp, json, now_ms, require_no_query, strict_query};
use crate::cloudflare_v4::{V4ResultInfo, paginated_response, success_response};
use axum::extract::{Path, Request, State};
use axum::response::Response;
use open_compute_core::{AccountId, ErrorCode, WorkflowOperationId, WorkflowVersionId};
use open_compute_storage::scheduler::WorkflowInstanceInspection;
use open_compute_storage::{
    VersionState, WorkerRepository, WorkflowDefinitionReservation, WorkflowVersion,
    decode_catalog_cursor,
};
use open_compute_workers::WorkflowController;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

pub(super) async fn list(
    State(state): State<HttpState>,
    Path(account_id): Path<String>,
    request: Request,
) -> Response {
    let (context, account, api) =
        match authenticated(&state, &request, V4Permission::Read, &account_id) {
            Ok(value) => value,
            Err(response) => return response,
        };
    let query = match list_query(&request) {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    let request_id = context.request_id();
    let result = tokio::task::spawn_blocking(move || {
        let definitions = all_definitions(&api, account, query.search.as_deref())?;
        let total_count = definitions.len();
        let offset = query
            .page
            .checked_sub(1)
            .and_then(|page| page.checked_mul(query.per_page))
            .ok_or(V4Error::InvalidRequest)?;
        let mut result = Vec::new();
        for definition in definitions.into_iter().skip(offset).take(query.per_page) {
            result.push(definition_result(&api, account, definition)?);
        }
        Ok::<_, V4Error>((result, total_count))
    })
    .await;
    match result {
        Ok(Ok((result, total_count))) => paginated_response(
            context,
            result,
            V4ResultInfo {
                page: query.page,
                per_page: query.per_page,
                count: total_count
                    .saturating_sub((query.page - 1) * query.per_page)
                    .min(query.per_page),
                total_count,
                total_pages: total_count.div_ceil(query.per_page),
            },
        ),
        Ok(Err(error)) => error_response(error, request_id),
        Err(_) => error_response(V4Error::Internal, request_id),
    }
}

pub(super) async fn get(
    State(state): State<HttpState>,
    Path((account_id, workflow_name)): Path<(String, String)>,
    request: Request,
) -> Response {
    let (context, account, api) =
        match authenticated(&state, &request, V4Permission::Read, &account_id) {
            Ok(value) => value,
            Err(response) => return response,
        };
    if let Err(error) = require_no_query(&request) {
        return error_response(error, context.request_id());
    }
    let request_id = context.request_id();
    let result = tokio::task::spawn_blocking(move || {
        let definition = definition(&api, account, &workflow_name)?;
        definition_result(&api, account, definition)
    })
    .await;
    match result {
        Ok(Ok(result)) => success_response(context, result),
        Ok(Err(error)) => error_response(error, request_id),
        Err(_) => error_response(V4Error::Internal, request_id),
    }
}

pub(super) async fn put(
    State(state): State<HttpState>,
    Path((account_id, workflow_name)): Path<(String, String)>,
    request: Request,
) -> Response {
    let (context, account, api) =
        match authenticated(&state, &request, V4Permission::ProductWrite, &account_id) {
            Ok(value) => value,
            Err(response) => return response,
        };
    if let Err(error) = require_no_query(&request).and_then(|()| valid_name(&workflow_name)) {
        return error_response(error, context.request_id());
    }
    let body: PutBody = match json(request, context.request_id()).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    if !valid_class(&body.class_name) {
        return error_response(V4Error::InvalidField("/class_name"), context.request_id());
    }
    if body.concurrency.is_some() {
        return error_response(V4Error::Unsupported, context.request_id());
    }
    if body.default_retention.is_some() {
        return error_response(V4Error::Unsupported, context.request_id());
    }
    if body.limits.is_some() {
        return error_response(V4Error::Unsupported, context.request_id());
    }
    if body.schedules.is_some() {
        return error_response(V4Error::Unsupported, context.request_id());
    }
    let request_id = context.request_id();
    let prepared = {
        let api = api.clone();
        let workflow_name = workflow_name.clone();
        let script_name = body.script_name.clone();
        tokio::task::spawn_blocking(move || {
            prepare_update(
                &api,
                account,
                &workflow_name,
                &script_name,
                &body.class_name,
                request_id,
            )
        })
        .await
    };
    let prepared = match prepared {
        Ok(Ok(value)) => value,
        Ok(Err(error)) => return error_response(error, request_id),
        Err(_) => return error_response(V4Error::Internal, request_id),
    };
    let reservation = prepared.reservation.clone();
    let version = match prepared.version {
        Some(version) => Ok(version),
        None => {
            api.create_reserved_version(
                account,
                prepared.definition.id,
                prepared.worker_version,
                prepared.class_name.clone(),
                match prepared.reservation {
                    Some(reservation) => reservation,
                    None => return error_response(V4Error::Internal, request_id),
                },
            )
            .await
        }
    };
    let version = match version {
        Ok(version) if version.state == VersionState::Ready => version,
        Ok(_) => return error_response(V4Error::Unavailable, request_id),
        Err(error) => {
            if let Some(reservation) = reservation {
                let repository = WorkflowRepository::new(api.storage().db());
                let cleanup_now = match now_ms() {
                    Ok(value) => value,
                    Err(cleanup_error) => return error_response(cleanup_error, request_id),
                };
                if repository
                    .release_definition_reservation(account, &reservation, cleanup_now)
                    .is_err()
                {
                    return error_response(V4Error::Internal, request_id);
                }
            }
            return error_response(V4Error::from(&error), request_id);
        }
    };
    match update_result(&api, prepared.definition, &version, &prepared.script_name) {
        Ok(result) => success_response(context, result),
        Err(error) => error_response(error, request_id),
    }
}

pub(super) async fn delete(
    State(state): State<HttpState>,
    Path((account_id, workflow_name)): Path<(String, String)>,
    request: Request,
) -> Response {
    let (context, account, api) =
        match authenticated(&state, &request, V4Permission::ProductWrite, &account_id) {
            Ok(value) => value,
            Err(response) => return response,
        };
    if let Err(error) = require_no_query(&request) {
        return error_response(error, context.request_id());
    }
    let request_id = context.request_id();
    let result = tokio::task::spawn_blocking(move || {
        let now = now_ms()?;
        let _admission = api
            .storage()
            .reserve_mutation(64 * 1024)
            .map_err(|error| V4Error::from(&error))?;
        let repository = WorkflowRepository::new(api.storage().db());
        let intent = repository
            .begin_definition_delete(account, &workflow_name, now)
            .map_err(|error| V4Error::from(&error))?;
        let definition = intent.definition.id;
        let instances = all_instances(&api, account, definition)?;
        let controller = WorkflowController::new(api.storage(), api.scheduler(), api.limits());
        for instance in instances {
            if let Err(error) = controller.delete(
                account,
                definition,
                instance.id,
                WorkflowOperationId::generate(),
                now,
            ) && error.code() != ErrorCode::WorkflowInstanceNotFound
            {
                return Err(V4Error::from(&error));
            }
        }
        repository
            .finish_definition_delete(account, &intent, now)
            .map_err(|error| V4Error::from(&error))?;
        Ok::<_, V4Error>(DeleteResult {
            success: Some(true),
            status: "ok",
        })
    })
    .await;
    match result {
        Ok(Ok(result)) => success_response(context, result),
        Ok(Err(error)) => error_response(error, request_id),
        Err(_) => error_response(V4Error::Internal, request_id),
    }
}

pub(super) async fn list_versions(
    State(state): State<HttpState>,
    Path((account_id, workflow_name)): Path<(String, String)>,
    request: Request,
) -> Response {
    let (context, account, api) =
        match authenticated(&state, &request, V4Permission::Read, &account_id) {
            Ok(value) => value,
            Err(response) => return response,
        };
    let query = match page_query(&request, 50) {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    let request_id = context.request_id();
    let result = tokio::task::spawn_blocking(move || {
        let definition = definition(&api, account, &workflow_name)?;
        let mut versions = all_versions(&api, account, definition.id)?;
        versions.sort_by(|left, right| right.version_number.cmp(&left.version_number));
        let total_count = versions.len();
        let offset = (query.page - 1)
            .checked_mul(query.per_page)
            .ok_or(V4Error::InvalidRequest)?;
        let result = versions
            .into_iter()
            .skip(offset)
            .take(query.per_page)
            .map(|version| version_result(&version))
            .collect::<Result<Vec<_>, _>>()?;
        Ok::<_, V4Error>((result, total_count))
    })
    .await;
    match result {
        Ok(Ok((result, total_count))) => paginated_response(
            context,
            result,
            V4ResultInfo {
                page: query.page,
                per_page: query.per_page,
                count: total_count
                    .saturating_sub((query.page - 1) * query.per_page)
                    .min(query.per_page),
                total_count,
                total_pages: total_count.div_ceil(query.per_page),
            },
        ),
        Ok(Err(error)) => error_response(error, request_id),
        Err(_) => error_response(V4Error::Internal, request_id),
    }
}

pub(super) async fn get_version(
    State(state): State<HttpState>,
    Path((account_id, workflow_name, version_id)): Path<(String, String, String)>,
    request: Request,
) -> Response {
    let (context, account, api) =
        match authenticated(&state, &request, V4Permission::Read, &account_id) {
            Ok(value) => value,
            Err(response) => return response,
        };
    if let Err(error) = require_no_query(&request) {
        return error_response(error, context.request_id());
    }
    let version_id: WorkflowVersionId = match version_id.parse() {
        Ok(value) => value,
        Err(_) => return error_response(V4Error::InvalidRequest, context.request_id()),
    };
    let request_id = context.request_id();
    let result = tokio::task::spawn_blocking(move || {
        let definition = definition(&api, account, &workflow_name)?;
        let version = WorkflowRepository::new(api.storage().db())
            .version(account, version_id)
            .map_err(|error| V4Error::from(&error))?;
        if version.target.definition_id != definition.id {
            return Err(V4Error::NotFound);
        }
        version_result(&version)
    })
    .await;
    match result {
        Ok(Ok(result)) => success_response(context, result),
        Ok(Err(error)) => error_response(error, request_id),
        Err(_) => error_response(V4Error::Internal, request_id),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PutBody {
    script_name: String,
    class_name: String,
    concurrency: Option<Value>,
    default_retention: Option<Value>,
    limits: Option<Value>,
    schedules: Option<Value>,
}

struct PreparedUpdate {
    definition: WorkflowDefinition,
    worker_version: open_compute_core::VersionId,
    class_name: String,
    script_name: String,
    version: Option<WorkflowVersion>,
    reservation: Option<WorkflowDefinitionReservation>,
}

fn prepare_update(
    api: &WorkflowApiState,
    account: AccountId,
    workflow_name: &str,
    script_name: &str,
    class_name: &str,
    request_id: open_compute_core::RequestId,
) -> Result<PreparedUpdate, V4Error> {
    let workers = WorkerRepository::new(api.storage().db());
    let worker = workers
        .list_workers(account)
        .map_err(|error| V4Error::from(&error))?
        .into_iter()
        .find(|worker| worker.name == script_name)
        .ok_or(V4Error::NotFound)?;
    let worker_version = worker.active_version_id.ok_or(V4Error::Conflict)?;
    let repository = WorkflowRepository::new(api.storage().db());
    let existing = repository
        .definitions(
            account,
            Some(workflow_name),
            None,
            CatalogSort::Name,
            CatalogDirection::Asc,
            None,
            100,
        )
        .map_err(|error| V4Error::from(&error))?
        .items
        .into_iter()
        .find(|definition| definition.name == workflow_name);
    let versions = match &existing {
        Some(definition) => all_versions(api, account, definition.id)?,
        None => Vec::new(),
    };
    if versions.iter().any(|version| {
        version.target.worker_id != worker.id && version.state == VersionState::Ready
    }) {
        return Err(V4Error::Conflict);
    }
    let reusable = existing.as_ref().and_then(|definition| {
        definition.current_version_id.and_then(|current| {
            versions.iter().find(|version| {
                version.target.workflow_version_id == current
                    && version.target.worker_version_id == worker_version
                    && version.target.class_name == class_name
                    && version.state == VersionState::Ready
            })
        })
    });
    let _admission = api
        .storage()
        .reserve_mutation(64 * 1024)
        .map_err(|error| V4Error::from(&error))?;
    let reservation = repository
        .reserve_definition(
            account,
            workflow_name,
            class_name,
            &request_id.to_string(),
            now_ms()?,
        )
        .map_err(|error| V4Error::from(&error))?;
    let definition = reservation.definition.clone();
    let version = reusable.cloned();
    let reservation = if version.is_some() {
        repository
            .release_definition_reservation(account, &reservation, now_ms()?)
            .map_err(|error| V4Error::from(&error))?;
        None
    } else {
        Some(reservation)
    };
    Ok(PreparedUpdate {
        definition,
        worker_version,
        class_name: class_name.to_owned(),
        script_name: script_name.to_owned(),
        version,
        reservation,
    })
}

fn all_definitions(
    api: &WorkflowApiState,
    account: AccountId,
    search: Option<&str>,
) -> Result<Vec<WorkflowDefinition>, V4Error> {
    let repository = WorkflowRepository::new(api.storage().db());
    let mut cursor = None;
    let mut result = Vec::new();
    loop {
        let page = repository
            .definitions(
                account,
                search,
                Some(ResourceState::Ready),
                CatalogSort::CreatedAt,
                CatalogDirection::Desc,
                cursor,
                1000,
            )
            .map_err(|error| V4Error::from(&error))?;
        result.extend(page.items);
        let Some(next) = page.next_cursor else {
            break;
        };
        cursor = Some(decode_catalog_cursor(&next).map_err(|_| V4Error::Internal)?);
    }
    Ok(result)
}

pub(super) fn all_versions(
    api: &WorkflowApiState,
    account: AccountId,
    definition: WorkflowId,
) -> Result<Vec<WorkflowVersion>, V4Error> {
    let repository = WorkflowRepository::new(api.storage().db());
    let mut after = 0;
    let mut result = Vec::new();
    loop {
        let page = repository
            .versions(account, definition, after, 1000)
            .map_err(|error| V4Error::from(&error))?;
        let count = page.len();
        if let Some(last) = page.last() {
            after = last.version_number;
        }
        result.extend(page);
        if count < 1000 {
            break;
        }
    }
    Ok(result)
}

fn definition_result(
    api: &WorkflowApiState,
    account: AccountId,
    definition: WorkflowDefinition,
) -> Result<DefinitionResult, V4Error> {
    let version = current_or_latest(api, account, &definition)?;
    let worker = WorkerRepository::new(api.storage().db())
        .get_worker(account, version.target.worker_id)
        .map_err(|error| V4Error::from(&error))?;
    let instances = all_instances(api, account, definition.id)?;
    let mut counts = BTreeMap::new();
    for instance in &instances {
        *counts
            .entry(status_name(
                instance.status,
                instance.durable.rollback_requested,
                instance.durable.pause_requested,
            ))
            .or_insert(0_u64) += 1;
    }
    let triggered_on = instances
        .iter()
        .map(|instance| instance.created_at_ms)
        .max()
        .map(iso_timestamp)
        .transpose()?;
    Ok(DefinitionResult {
        name: definition.name,
        id: workflow_id(definition.id),
        created_on: iso_timestamp(definition.created_at_ms)?,
        modified_on: iso_timestamp(definition.updated_at_ms)?,
        script_name: worker.name,
        class_name: version.target.class_name,
        triggered_on,
        instances: counts,
    })
}

pub(super) fn all_instances(
    api: &WorkflowApiState,
    account: AccountId,
    definition: WorkflowId,
) -> Result<Vec<WorkflowInstanceInspection>, V4Error> {
    let mut after = None;
    let mut result = Vec::new();
    loop {
        let page = api
            .scheduler()
            .inspect_workflow_instances(account, definition, after, 1000, now_ms()?)
            .map_err(|error| V4Error::from(&error))?;
        let count = page.len();
        after = page.last().map(|instance| instance.id);
        result.extend(page);
        if count < 1000 {
            break;
        }
    }
    Ok(result)
}

fn current_or_latest(
    api: &WorkflowApiState,
    account: AccountId,
    definition: &WorkflowDefinition,
) -> Result<WorkflowVersion, V4Error> {
    if let Some(version) = definition.current_version_id {
        return WorkflowRepository::new(api.storage().db())
            .version(account, version)
            .map_err(|error| V4Error::from(&error));
    }
    all_versions(api, account, definition.id)?
        .into_iter()
        .max_by_key(|version| version.version_number)
        .ok_or(V4Error::Unavailable)
}

fn update_result(
    api: &WorkflowApiState,
    definition: WorkflowDefinition,
    version: &WorkflowVersion,
    script_name: &str,
) -> Result<UpdateResult, V4Error> {
    let definition = WorkflowRepository::new(api.storage().db())
        .definition(definition.account_id, definition.id)
        .map_err(|error| V4Error::from(&error))?;
    Ok(UpdateResult {
        version_id: version.target.workflow_version_id.to_string(),
        name: definition.name,
        id: definition.id.to_string(),
        created_on: iso_timestamp(definition.created_at_ms)?,
        modified_on: iso_timestamp(definition.updated_at_ms)?,
        script_name: script_name.to_owned(),
        class_name: version.target.class_name.clone(),
        triggered_on: None,
        is_deleted: 0,
        terminator_running: 0,
    })
}

fn version_result(version: &WorkflowVersion) -> Result<VersionResult, V4Error> {
    let timestamp = iso_timestamp(version.created_at_ms)?;
    Ok(VersionResult {
        created_on: timestamp.clone(),
        modified_on: timestamp,
        id: version.target.workflow_version_id.to_string(),
        workflow_id: version.target.definition_id.to_string(),
        class_name: version.target.class_name.clone(),
        has_dag: false,
        language: "javascript",
    })
}

fn list_query(request: &Request) -> Result<ListQuery, V4Error> {
    let values = strict_query(request)?;
    if values
        .keys()
        .any(|key| !matches!(key.as_str(), "page" | "per_page" | "search"))
    {
        return Err(V4Error::InvalidRequest);
    }
    let page = number(&values, "page", 1, 1, usize::MAX)?;
    let per_page = number(&values, "per_page", 10, 1, 100)?;
    let search = values.get("search").cloned();
    if search
        .as_deref()
        .is_some_and(|value| valid_name(value).is_err())
    {
        return Err(V4Error::InvalidRequest);
    }
    Ok(ListQuery {
        page,
        per_page,
        search,
    })
}

fn page_query(request: &Request, default: usize) -> Result<PageQuery, V4Error> {
    let values = strict_query(request)?;
    if values
        .keys()
        .any(|key| !matches!(key.as_str(), "page" | "per_page"))
    {
        return Err(V4Error::InvalidRequest);
    }
    Ok(PageQuery {
        page: number(&values, "page", 1, 1, usize::MAX)?,
        per_page: number(&values, "per_page", default, 1, 100)?,
    })
}

fn number(
    values: &BTreeMap<String, String>,
    key: &str,
    default: usize,
    minimum: usize,
    maximum: usize,
) -> Result<usize, V4Error> {
    let value = values.get(key).map_or(Ok(default), |value| {
        value.parse().map_err(|_| V4Error::InvalidRequest)
    })?;
    (minimum..=maximum)
        .contains(&value)
        .then_some(value)
        .ok_or(V4Error::InvalidRequest)
}

fn valid_class(value: &str) -> bool {
    let bytes = value.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 128
        && !value.starts_with("__")
        && (bytes[0].is_ascii_alphabetic() || matches!(bytes[0], b'_' | b'$'))
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'$'))
}

struct ListQuery {
    page: usize,
    per_page: usize,
    search: Option<String>,
}

struct PageQuery {
    page: usize,
    per_page: usize,
}

#[derive(Serialize)]
struct DefinitionResult {
    name: String,
    id: String,
    created_on: String,
    modified_on: String,
    script_name: String,
    class_name: String,
    triggered_on: Option<String>,
    instances: BTreeMap<&'static str, u64>,
}

#[derive(Serialize)]
struct UpdateResult {
    version_id: String,
    name: String,
    id: String,
    created_on: String,
    modified_on: String,
    script_name: String,
    class_name: String,
    triggered_on: Option<String>,
    is_deleted: u8,
    terminator_running: u8,
}

#[derive(Serialize)]
struct VersionResult {
    created_on: String,
    modified_on: String,
    id: String,
    workflow_id: String,
    class_name: String,
    has_dag: bool,
    language: &'static str,
}

#[derive(Serialize)]
struct DeleteResult {
    success: Option<bool>,
    status: &'static str,
}
