//! Authenticated instance lifecycle uses the same controller as tenant bindings.

use super::*;
use open_compute_core::WorkflowOperationId;
use open_compute_storage::scheduler::WorkflowInstanceAction;
use open_compute_workers::{WorkflowController, WorkflowEventInput};

pub(super) fn routes() -> Router<HttpState> {
    Router::new()
        .route(
            "/v1/accounts/{account}/workflows/{definition}/instances/{instance}",
            get(inspect),
        )
        .route(
            "/v1/accounts/{account}/workflows/{definition}/instances/{instance}/events",
            post(send_event),
        )
        .route(
            "/v1/accounts/{account}/workflows/{definition}/instances/{instance}/{action}",
            post(modify),
        )
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EmptyBody {}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EventBody {
    #[serde(rename = "type")]
    event_type: String,
    #[serde(rename = "payloadBase64")]
    payload_base64: String,
}

async fn inspect(
    State(state): State<HttpState>,
    Path((account, definition, instance)): Path<(String, String, String)>,
    request: Request,
) -> Response {
    let id = request_id(&request);
    let api = match authorized(&state, &request) {
        Ok(api) => api,
        Err(error) => return failure(&error, id),
    };
    let (account, definition, instance) = match instance_ids(&account, &definition, &instance) {
        Ok(ids) => ids,
        Err(error) => return failure(&error, id),
    };
    response(
        tokio::task::spawn_blocking(move || {
            WorkflowController::new(&api.storage, &api.scheduler, &api.limits).inspect(
                account,
                definition,
                instance,
                now_ms(),
            )
        })
        .await,
        id,
        StatusCode::OK,
    )
}

async fn modify(
    State(state): State<HttpState>,
    Path((account, definition, instance, action)): Path<(String, String, String, String)>,
    request: Request,
) -> Response {
    let id = request_id(&request);
    let api = match authorized(&state, &request) {
        Ok(api) => api,
        Err(error) => return failure(&error, id),
    };
    let (account, definition, instance) = match instance_ids(&account, &definition, &instance) {
        Ok(ids) => ids,
        Err(error) => return failure(&error, id),
    };
    let modifier = match action.as_str() {
        "pause" => Some(WorkflowInstanceAction::Pause),
        "resume" => Some(WorkflowInstanceAction::Resume),
        "terminate" => Some(WorkflowInstanceAction::Terminate),
        "restart" => None,
        _ => return failure(&error(ErrorCode::WorkflowMethodUnsupported), id),
    };
    if let Err(error) = read_json::<EmptyBody>(request, 1024).await {
        return failure(&error, id);
    }
    let metrics = state.metrics().clone();
    response(
        tokio::task::spawn_blocking(move || {
            let controller = WorkflowController::new(&api.storage, &api.scheduler, &api.limits);
            let result = if let Some(modifier) = modifier {
                controller.modify(account, definition, instance, modifier, now_ms())
            } else {
                controller.restart(
                    account,
                    definition,
                    instance,
                    WorkflowOperationId::generate(),
                    None,
                    now_ms(),
                )
            };
            metrics.workflow_lifecycle(&action, result.is_ok());
            result?;
            Ok(serde_json::json!({"ok":true}))
        })
        .await,
        id,
        StatusCode::OK,
    )
}

async fn send_event(
    State(state): State<HttpState>,
    Path((account, definition, instance)): Path<(String, String, String)>,
    request: Request,
) -> Response {
    let id = request_id(&request);
    let api = match authorized(&state, &request) {
        Ok(api) => api,
        Err(error) => return failure(&error, id),
    };
    let (account, definition, instance) = match instance_ids(&account, &definition, &instance) {
        Ok(ids) => ids,
        Err(error) => return failure(&error, id),
    };
    let body: EventBody = match read_json(request, 2 * 1024 * 1024 + 8192).await {
        Ok(body) => body,
        Err(error) => return failure(&error, id),
    };
    let metrics = state.metrics().clone();
    response(
        tokio::task::spawn_blocking(move || {
            let result = WorkflowController::new(&api.storage, &api.scheduler, &api.limits)
                .send_event(
                    account,
                    definition,
                    instance,
                    WorkflowEventInput {
                        operation_id: WorkflowOperationId::generate(),
                        event_type: &body.event_type,
                        payload_base64: &body.payload_base64,
                    },
                    now_ms(),
                );
            metrics.workflow_event(result.as_ref().err().map(PlatformError::code));
            result?;
            Ok(serde_json::json!({"ok":true}))
        })
        .await,
        id,
        StatusCode::OK,
    )
}

fn instance_ids(
    account: &str,
    definition: &str,
    instance: &str,
) -> Result<(AccountId, WorkflowId, WorkflowInstanceId), PlatformError> {
    Ok((parse(account)?, parse(definition)?, parse(instance)?))
}
