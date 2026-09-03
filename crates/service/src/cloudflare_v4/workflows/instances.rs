//! Workflow instance creation, inspection, lifecycle, and event routes.

mod query;
use query::*;

use super::cursor::Position;
use super::definitions::all_instances;
use super::*;
use crate::cloudflare_v4::storage::{
    iso_timestamp, json, json_with_limit, now_ms, require_no_query,
};
use crate::cloudflare_v4::{result_info_response, success_response};
use axum::extract::{Path, Request, State};
use axum::response::Response;
use open_compute_core::WorkflowOperationId;
use open_compute_core::workflow::{WORKFLOW_MAX_DURATION_MS, WorkflowRetention, duration_ms};
use open_compute_storage::scheduler::WorkflowInstanceInspection;
use open_compute_storage::{WorkflowRepository, WorkflowReservation};
use open_compute_workers::{WorkflowController, WorkflowCreateInput, WorkflowEventInput};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json as json_value};

const MAX_BATCH_BODY: usize = 100 * 1024 * 1024;

pub(super) async fn create(
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
    let body: CreateBody = match json(request, context.request_id()).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let prepared = match prepare_create(body, api.limits()) {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    let request_id = context.request_id();
    let result = tokio::task::spawn_blocking(move || {
        let definition = definition(&api, account, &workflow_name)?;
        let operation = WorkflowOperationId::generate();
        let identity = WorkflowController::new(api.storage(), api.scheduler(), api.limits())
            .create(
                account,
                definition.id,
                operation,
                prepared.instance_id.as_deref(),
                WorkflowCreateInput {
                    payload_base64: &prepared.payload,
                    retention: prepared.retention.as_ref(),
                    schedule: None,
                },
                now_ms()?,
            )
            .map_err(|error| V4Error::from(&error))?;
        Ok::<_, V4Error>(creation_result(&identity))
    })
    .await;
    match result {
        Ok(Ok(result)) => success_response(context, result),
        Ok(Err(error)) => error_response(error, request_id),
        Err(_) => error_response(V4Error::Internal, request_id),
    }
}

pub(super) async fn batch(
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
    let body: Vec<CreateBody> =
        match json_with_limit::<Vec<CreateBody>>(request, context.request_id(), MAX_BATCH_BODY)
            .await
        {
            Ok(value) if (1..=100).contains(&value.len()) => value,
            Ok(_) => return error_response(V4Error::InvalidRequest, context.request_id()),
            Err(response) => return response,
        };
    let prepared = match body
        .into_iter()
        .map(|body| prepare_create(body, api.limits()))
        .collect::<Result<Vec<_>, _>>()
    {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    let request_id = context.request_id();
    let result = tokio::task::spawn_blocking(move || {
        let definition = definition(&api, account, &workflow_name)?;
        let operations = (0..prepared.len())
            .map(|_| WorkflowOperationId::generate())
            .collect::<Vec<_>>();
        let requests = prepared
            .iter()
            .zip(&operations)
            .map(|(prepared, operation)| {
                (
                    *operation,
                    prepared.instance_id.as_deref(),
                    WorkflowCreateInput {
                        payload_base64: &prepared.payload,
                        retention: prepared.retention.as_ref(),
                        schedule: None,
                    },
                )
            })
            .collect::<Vec<_>>();
        let identities = WorkflowController::new(api.storage(), api.scheduler(), api.limits())
            .create_batch(
                account,
                definition.id,
                WorkflowOperationId::generate(),
                &requests,
                now_ms()?,
            )
            .map_err(|error| V4Error::from(&error))?;
        Ok::<_, V4Error>(identities.iter().map(creation_result).collect::<Vec<_>>())
    })
    .await;
    match result {
        Ok(Ok(result)) => success_response(context, result),
        Ok(Err(error)) => error_response(error, request_id),
        Err(_) => error_response(V4Error::Internal, request_id),
    }
}

pub(super) async fn list(
    State(state): State<HttpState>,
    Path((account_id, workflow_name)): Path<(String, String)>,
    request: Request,
) -> Response {
    let (context, account, api) =
        match authenticated(&state, &request, V4Permission::Read, &account_id) {
            Ok(value) => value,
            Err(response) => return response,
        };
    let query = match ListQuery::parse(&request) {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    let response_page = query.page;
    let response_per_page = query.per_page;
    let request_id = context.request_id();
    let result = tokio::task::spawn_blocking(move || {
        let definition = definition(&api, account, &workflow_name)?;
        let now = now_ms()?;
        let cursor = query
            .cursor
            .as_deref()
            .map(|cursor| {
                cursor::open(
                    api.storage(),
                    cursor,
                    account,
                    &workflow_name,
                    &query.binding(),
                    now,
                )
            })
            .transpose()?;
        let mut instances = all_instances(&api, account, definition.id)?;
        instances.retain(|instance| query.matches(instance));
        instances.sort_by(|left, right| {
            (left.created_at_ms, left.id.to_string())
                .cmp(&(right.created_at_ms, right.id.to_string()))
        });
        if query.descending() {
            instances.reverse();
        }
        if let Some(cursor) = cursor {
            instances.retain(|instance| query.after(instance, &cursor));
        } else if query.page > 1 {
            instances = instances
                .into_iter()
                .skip((query.page - 1).saturating_mul(query.per_page))
                .collect();
        }
        let total_count = instances.len();
        let has_more = instances.len() > query.per_page;
        instances.truncate(query.per_page);
        let result = instances
            .iter()
            .map(|instance| instance_result(&api, definition.id, instance))
            .collect::<Result<Vec<_>, _>>()?;
        let cursor = if has_more {
            instances
                .last()
                .map(|instance| {
                    cursor::seal(
                        api.storage(),
                        account,
                        &workflow_name,
                        &query.binding(),
                        &Position {
                            created_at_ms: instance.created_at_ms,
                            instance_id: instance.id,
                        },
                        now.checked_add(CURSOR_LIFETIME_MS)
                            .ok_or(V4Error::Internal)?,
                    )
                })
                .transpose()?
        } else {
            None
        };
        Ok::<_, V4Error>((result, cursor, total_count))
    })
    .await;
    match result {
        Ok(Ok((result, cursor, total_count))) => {
            let count = result.len();
            result_info_response(
                context,
                result,
                InstanceResultInfo {
                    count,
                    cursor,
                    page: response_page,
                    per_page: response_per_page,
                    total_count,
                    total_pages: total_count.div_ceil(response_per_page),
                },
            )
        }
        Ok(Err(error)) => error_response(error, request_id),
        Err(_) => error_response(V4Error::Internal, request_id),
    }
}

pub(super) async fn get(
    State(state): State<HttpState>,
    Path((account_id, workflow_name, instance_id)): Path<(String, String, String)>,
    request: Request,
) -> Response {
    let (context, account, api) =
        match authenticated(&state, &request, V4Permission::Read, &account_id) {
            Ok(value) => value,
            Err(response) => return response,
        };
    let query = match DetailQuery::parse(&request) {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    let request_id = context.request_id();
    let result = tokio::task::spawn_blocking(move || {
        let definition = definition(&api, account, &workflow_name)?;
        let reservation = reservation(&api, definition.id, &instance_id)?;
        let now = now_ms()?;
        WorkflowController::new(api.storage(), api.scheduler(), api.limits())
            .inspect(
                account,
                definition.id,
                reservation.identity.instance_id,
                now,
            )
            .map_err(|error| V4Error::from(&error))?;
        detail_result(&api, &reservation, query)
    })
    .await;
    match result {
        Ok(Ok(result)) => success_response(context, result),
        Ok(Err(error)) => error_response(error, request_id),
        Err(_) => error_response(V4Error::Internal, request_id),
    }
}

pub(super) async fn status(
    State(state): State<HttpState>,
    Path((account_id, workflow_name, instance_id)): Path<(String, String, String)>,
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
    let body: StatusBody = match json(request, context.request_id()).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let action = match body.validate() {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    let request_id = context.request_id();
    let result = tokio::task::spawn_blocking(move || {
        let definition = definition(&api, account, &workflow_name)?;
        let reservation = reservation(&api, definition.id, &instance_id)?;
        let now = now_ms()?;
        let controller = WorkflowController::new(api.storage(), api.scheduler(), api.limits());
        match action {
            StatusAction::Modify(action) => controller.modify(
                account,
                definition.id,
                reservation.identity.instance_id,
                action,
                now,
            ),
            StatusAction::Rollback => controller.rollback(
                account,
                definition.id,
                reservation.identity.instance_id,
                now,
            ),
            StatusAction::Restart(from) => controller.restart(
                account,
                definition.id,
                reservation.identity.instance_id,
                WorkflowOperationId::generate(),
                from,
                now,
            ),
        }
        .map_err(|error| V4Error::from(&error))?;
        let instance = api
            .scheduler()
            .workflow_instance(reservation.identity.instance_id)
            .map_err(|error| V4Error::from(&error))?
            .ok_or(V4Error::NotFound)?;
        Ok::<_, V4Error>(StatusResult {
            status: status_name(
                instance.state,
                instance.durable.rollback_requested,
                instance.durable.pause_requested,
            ),
            timestamp: iso_timestamp(now)?,
        })
    })
    .await;
    match result {
        Ok(Ok(result)) => success_response(context, result),
        Ok(Err(error)) => error_response(error, request_id),
        Err(_) => error_response(V4Error::Internal, request_id),
    }
}

pub(super) async fn event(
    State(state): State<HttpState>,
    Path((account_id, workflow_name, instance_id, event_type)): Path<(
        String,
        String,
        String,
        String,
    )>,
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
    if open_compute_core::workflow::validate_workflow_event_type(&event_type).is_err() {
        return error_response(V4Error::InvalidRequest, context.request_id());
    }
    let body: Value = match json(request, context.request_id()).await {
        Ok(Value::Object(fields)) => Value::Object(fields),
        Ok(_) => return error_response(V4Error::InvalidRequest, context.request_id()),
        Err(response) => return response,
    };
    let payload = match value::encode(&body) {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    let request_id = context.request_id();
    let result = tokio::task::spawn_blocking(move || {
        let definition = definition(&api, account, &workflow_name)?;
        let reservation = reservation(&api, definition.id, &instance_id)?;
        let now = now_ms()?;
        WorkflowController::new(api.storage(), api.scheduler(), api.limits())
            .send_event(
                account,
                definition.id,
                reservation.identity.instance_id,
                WorkflowEventInput {
                    operation_id: WorkflowOperationId::generate(),
                    event_type: &event_type,
                    payload_base64: &payload,
                },
                now,
            )
            .map_err(|error| V4Error::from(&error))?;
        Ok::<_, V4Error>(EventResult {
            instance_id: reservation.identity.external_instance_id,
            timestamp: iso_timestamp(now)?,
        })
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
struct CreateBody {
    instance_id: Option<String>,
    instance_retention: Option<RetentionBody>,
    location_hint: Option<String>,
    params: Option<Value>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RetentionBody {
    success_retention: Option<Value>,
    error_retention: Option<Value>,
}

struct PreparedCreate {
    instance_id: Option<String>,
    payload: String,
    retention: Option<WorkflowRetention>,
}

fn prepare_create(
    body: CreateBody,
    limits: &open_compute_core::WorkflowsConfig,
) -> Result<PreparedCreate, V4Error> {
    if body.location_hint.is_some() {
        return Err(V4Error::Unsupported);
    }
    if let Some(instance_id) = body.instance_id.as_deref() {
        valid_instance_id(instance_id)?;
        if instance_id.starts_with("cf_")
            && instance_id.len() == 67
            && instance_id[3..]
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        {
            return Err(V4Error::InvalidField("/instance_id"));
        }
    }
    let payload = value::parameter(body.params.unwrap_or_else(|| json_value!({})))?;
    let retention = body
        .instance_retention
        .map(|retention| {
            let mut resolved = limits.default_retention.clone();
            if let Some(value) = retention.success_retention {
                resolved.success_retention_ms = duration_ms(&value, WORKFLOW_MAX_DURATION_MS)
                    .map_err(|_| V4Error::InvalidField("/instance_retention/success_retention"))?;
            }
            if let Some(value) = retention.error_retention {
                resolved.error_retention_ms = duration_ms(&value, WORKFLOW_MAX_DURATION_MS)
                    .map_err(|_| V4Error::InvalidField("/instance_retention/error_retention"))?;
            }
            resolved
                .validate()
                .map_err(|_| V4Error::InvalidField("/instance_retention"))?;
            Ok::<_, V4Error>(resolved)
        })
        .transpose()?;
    Ok(PreparedCreate {
        instance_id: body.instance_id,
        payload,
        retention,
    })
}

fn reservation(
    api: &WorkflowApiState,
    definition: WorkflowId,
    instance_id: &str,
) -> Result<WorkflowReservation, V4Error> {
    valid_instance_id(instance_id)?;
    WorkflowRepository::new(api.storage().db())
        .find_instance(definition, instance_id)
        .map_err(|error| V4Error::from(&error))
}

fn creation_result(identity: &open_compute_storage::WorkflowInstanceIdentity) -> CreationResult {
    CreationResult {
        id: identity.external_instance_id.clone(),
        workflow_id: identity.target.definition_id.to_string(),
        version_id: identity.target.workflow_version_id.to_string(),
        status: "queued",
        trigger_source: "api",
    }
}

fn instance_result(
    api: &WorkflowApiState,
    definition: WorkflowId,
    inspection: &WorkflowInstanceInspection,
) -> Result<InstanceResult, V4Error> {
    let reservation = WorkflowRepository::new(api.storage().db())
        .reservation(inspection.id)
        .map_err(|error| V4Error::from(&error))?
        .filter(|reservation| reservation.identity.target.definition_id == definition)
        .ok_or(V4Error::Internal)?;
    let record = api
        .scheduler()
        .workflow_instance(inspection.id)
        .map_err(|error| V4Error::from(&error))?
        .ok_or(V4Error::Internal)?;
    Ok(InstanceResult {
        id: inspection.external_instance_id.clone(),
        created_on: iso_timestamp(inspection.created_at_ms)?,
        modified_on: iso_timestamp(record.updated_at_ms)?,
        started_on: inspection
            .durable
            .has_activated
            .then(|| iso_timestamp(inspection.created_at_ms))
            .transpose()?,
        ended_on: inspection.terminal_at_ms.map(iso_timestamp).transpose()?,
        workflow_id: reservation.identity.target.definition_id.to_string(),
        version_id: inspection.workflow_version_id.to_string(),
        status: status_name(
            inspection.status,
            inspection.durable.rollback_requested,
            inspection.durable.pause_requested,
        ),
        trigger_source: if reservation.identity.schedule.is_some() {
            "cron"
        } else {
            "api"
        },
    })
}

fn detail_result(
    api: &WorkflowApiState,
    reservation: &WorkflowReservation,
    query: DetailQuery,
) -> Result<DetailResult, V4Error> {
    let record = api
        .scheduler()
        .workflow_instance(reservation.identity.instance_id)
        .map_err(|error| V4Error::from(&error))?
        .ok_or(V4Error::NotFound)?;
    let mut steps = api
        .scheduler()
        .workflow_steps(reservation.identity.instance_id, None, 1000)
        .map_err(|error| V4Error::from(&error))?;
    if query.order == Direction::Desc {
        steps.reverse();
    }
    let step_start = iso_timestamp(reservation.identity.created_at_ms)?;
    let step_end = record.terminal_at_ms.map(iso_timestamp).transpose()?;
    let steps = if query.simple {
        Vec::new()
    } else {
        steps
            .into_iter()
            .map(|step| {
                json_value!({
                    "name": step.name,
                    "start": step_start,
                    "end": step_end,
                    "attempts": [],
                    "config": {"retries":{"limit":0,"delay":0},"timeout":0},
                    "output": Value::Null,
                    "success": if step.state == "complete" { Some(true) } else if step.state == "failed" { Some(false) } else { None },
                    "type": if step.kind == "wait_event" { "waitForEvent" } else if step.kind.starts_with("sleep") { "sleep" } else { "step" },
                })
            })
            .collect::<Vec<_>>()
    };
    let status = status_name(
        record.state,
        record.durable.rollback_requested,
        record.durable.pause_requested,
    );
    Ok(DetailResult {
        params: value::decode(&record.input_json)?,
        trigger: Trigger {
            source: if reservation.identity.schedule.is_some() {
                "cron"
            } else {
                "api"
            },
        },
        version_id: record.identity.target.workflow_version_id.to_string(),
        queued: iso_timestamp(record.identity.created_at_ms)?,
        start: record
            .durable
            .has_activated
            .then(|| iso_timestamp(record.identity.created_at_ms))
            .transpose()?,
        end: record.terminal_at_ms.map(iso_timestamp).transpose()?,
        step_count: record.durable.registered_step_count,
        steps,
        success: match record.state {
            WorkflowState::Complete => Some(true),
            WorkflowState::Errored | WorkflowState::Terminated => Some(false),
            _ => None,
        },
        error: record.error.map(|error| ErrorResult {
            name: error.name,
            message: error.message,
        }),
        status,
        output: record
            .output_json
            .as_deref()
            .map(value::decode)
            .transpose()?,
        rollback: None,
        schedule: reservation
            .identity
            .schedule
            .as_ref()
            .map(|schedule| ScheduleResult {
                cron: schedule.cron.clone(),
                scheduled_time: schedule.scheduled_time,
            }),
    })
}

#[derive(Serialize)]
struct CreationResult {
    id: String,
    workflow_id: String,
    version_id: String,
    status: &'static str,
    trigger_source: &'static str,
}

#[derive(Serialize)]
struct InstanceResult {
    id: String,
    created_on: String,
    modified_on: String,
    started_on: Option<String>,
    ended_on: Option<String>,
    workflow_id: String,
    version_id: String,
    status: &'static str,
    trigger_source: &'static str,
}

#[derive(Serialize)]
struct InstanceResultInfo {
    count: usize,
    cursor: Option<String>,
    page: usize,
    per_page: usize,
    total_count: usize,
    total_pages: usize,
}

#[derive(Serialize)]
struct DetailResult {
    params: Value,
    trigger: Trigger,
    #[serde(rename = "versionId")]
    version_id: String,
    queued: String,
    start: Option<String>,
    end: Option<String>,
    step_count: u32,
    steps: Vec<Value>,
    success: Option<bool>,
    error: Option<ErrorResult>,
    status: &'static str,
    output: Option<Value>,
    rollback: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    schedule: Option<ScheduleResult>,
}

#[derive(Serialize)]
struct Trigger {
    source: &'static str,
}

#[derive(Serialize)]
struct ErrorResult {
    name: String,
    message: String,
}

#[derive(Serialize)]
struct ScheduleResult {
    cron: String,
    #[serde(rename = "scheduledTime")]
    scheduled_time: i64,
}

#[derive(Serialize)]
struct StatusResult {
    status: &'static str,
    timestamp: String,
}

#[derive(Serialize)]
struct EventResult {
    #[serde(rename = "instanceId")]
    instance_id: String,
    timestamp: String,
}
