//! Authenticated Workflow caller methods and private run/step persistence protocol.

use crate::metrics::{MetricsRegistry, WorkflowOutcome};
use axum::body::to_bytes;
use axum::extract::Request;
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode};
use axum::response::{IntoResponse as _, Response};
use open_compute_core::workflow::WorkflowStepDeclaration;
use open_compute_core::{
    BindingId, DeploymentId, ErrorCode, PlatformError, SchedulerClock as _, WorkflowFence,
    WorkflowInstanceId, WorkflowsConfig,
};
use open_compute_runtime::GenerationAuthRegistry;
use open_compute_storage::scheduler::{
    WorkflowStepAttempt, WorkflowStepGrant, WorkflowStepOutcome,
};
use open_compute_storage::{PlatformStorage, SchedulerStore, WorkflowRepository};
use open_compute_workers::WorkflowController;
use serde::Deserialize;
use serde_json::Value;
use std::sync::Arc;
use std::time::Duration;

const MAX_BODY: usize = 2 * 1024 * 1024 + 8192;
const TIMEOUT: Duration = Duration::from_secs(30);

/// Bounded private Workflow data plane composed with platform-owned authorities.
#[derive(Clone, Debug)]
pub struct WorkflowBindingService {
    storage: Arc<PlatformStorage>,
    scheduler: Arc<SchedulerStore>,
    config: WorkflowsConfig,
    concurrency: Arc<tokio::sync::Semaphore>,
    metrics: Option<Arc<MetricsRegistry>>,
}

impl WorkflowBindingService {
    /// Validate local policy and reserve a bounded private request lane.
    pub fn new(
        storage: Arc<PlatformStorage>,
        scheduler: Arc<SchedulerStore>,
        config: WorkflowsConfig,
    ) -> Result<Self, PlatformError> {
        config.validate()?;
        Ok(Self {
            storage,
            scheduler,
            concurrency: Arc::new(tokio::sync::Semaphore::new(
                config.max_in_flight_requests as usize,
            )),
            config,
            metrics: None,
        })
    }

    /// Attach fixed Workflow series without accepting tenant-controlled metric labels.
    #[must_use]
    pub fn with_metrics(mut self, metrics: Arc<MetricsRegistry>) -> Self {
        self.metrics = Some(metrics);
        self
    }

    /// Handle an already authenticated request, rechecking generation after reading its bounded body.
    pub async fn handle(&self, request: Request, auth: GenerationAuthRegistry) -> Response {
        let Ok(permit) = self.concurrency.clone().try_acquire_owned() else {
            return response_error(ErrorCode::WorkflowRuntimeUnavailable);
        };
        if request
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            != Some("application/json")
        {
            return response_error(ErrorCode::WorkflowMethodUnsupported);
        }
        let (parts, body) = request.into_parts();
        let bytes = match tokio::time::timeout(TIMEOUT, to_bytes(body, MAX_BODY)).await {
            Ok(Ok(bytes)) => bytes,
            Ok(Err(_)) => return response_error(ErrorCode::WorkflowResultTooLarge),
            Err(_) => return response_error(ErrorCode::WorkflowRuntimeUnavailable),
        };
        let service = self.clone();
        let task = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let token = header(&parts.headers, "x-open-compute-binding-token")?;
            let generation = header(&parts.headers, "x-open-compute-startup-generation")?;
            let body: Value = serde_json::from_slice(&bytes)
                .map_err(|_| failure(ErrorCode::WorkflowSerializationUnsupported))?;
            auth.with_authorized(token, generation, || {
                service.execute(
                    parts.uri.path(),
                    &parts.headers,
                    body,
                    open_compute_core::SystemSchedulerClock.wall_time_ms(),
                )
            })
            .unwrap_or_else(|| Err(failure(ErrorCode::WorkflowRunStale)))
        });
        match tokio::time::timeout(TIMEOUT, task).await {
            Ok(Ok(Ok(value))) => axum::Json(value).into_response(),
            Ok(Ok(Err(error))) => response_error(error.code()),
            Ok(Err(_)) | Err(_) => response_error(ErrorCode::WorkflowRuntimeUnavailable),
        }
    }

    fn execute(
        &self,
        path: &str,
        headers: &HeaderMap,
        body: Value,
        now_ms: i64,
    ) -> Result<Value, PlatformError> {
        if let Some(operation) = path.strip_prefix("/internal/workflows/runs/") {
            return self.run(operation, body, now_ms);
        }
        let tail = path
            .strip_prefix("/internal/bindings/v1/workflow/")
            .ok_or_else(|| failure(ErrorCode::WorkflowMethodUnsupported))?;
        let (binding, operation) = tail
            .split_once('/')
            .ok_or_else(|| failure(ErrorCode::WorkflowMethodUnsupported))?;
        let binding: BindingId = binding
            .parse()
            .map_err(|_| failure(ErrorCode::WorkflowBindingStale))?;
        let deployment: DeploymentId = header(headers, "x-open-compute-deployment-id")?
            .parse()
            .map_err(|_| failure(ErrorCode::WorkflowBindingStale))?;
        let digest = header(headers, "x-open-compute-descriptor-sha256")?;
        if digest.len() != 64
            || !digest
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(failure(ErrorCode::WorkflowBindingStale));
        }
        let mut expected = [0; 32];
        hex::decode_to_slice(digest, &mut expected)
            .map_err(|_| failure(ErrorCode::WorkflowBindingStale))?;
        let repository = WorkflowRepository::new(self.storage.db());
        let (account, binding) = repository.authorize_binding(binding, deployment, &expected)?;
        let definition = binding.descriptor.definition_id;
        let capability = binding.descriptor.capability_version;
        if capability != 1 {
            return Err(failure(ErrorCode::WorkflowCapabilityMismatch));
        }
        let controller = WorkflowController::new(&self.storage, &self.scheduler, &self.config);
        if matches!(
            operation,
            "create" | "send-event" | "pause" | "resume" | "terminate" | "restart"
        ) && header(headers, "x-open-compute-workflow-do-context")? != "0"
        {
            return Err(failure(ErrorCode::WorkflowDoOutputGateUnsupported));
        }
        match operation {
            "create" => {
                let request: CreateRequest = decode(body)?;
                let retention = request
                    .retention
                    .as_ref()
                    .map(|value| {
                        open_compute_core::workflow::WorkflowRetention::resolve(
                            value,
                            &self.config.default_retention,
                        )
                    })
                    .transpose()?;
                let result = controller.create(
                    account,
                    definition,
                    request.id.as_deref(),
                    open_compute_workers::WorkflowCreateInput {
                        payload_json: &request.payload_json,
                        retention: retention.as_ref(),
                    },
                    now_ms,
                );
                if let Some(metrics) = &self.metrics {
                    metrics.workflow_created(if result.is_ok() {
                        WorkflowOutcome::Success
                    } else {
                        WorkflowOutcome::Error
                    });
                }
                let identity = result?;
                Ok(
                    serde_json::json!({"id":identity.external_instance_id,"instanceId":identity.instance_id}),
                )
            }
            "get" | "status" => {
                let (id, external) = if operation == "get" {
                    let request: InstanceRequest = decode(body)?;
                    let reservation = repository.find_instance(definition, &request.id)?;
                    (reservation.identity.instance_id, Some(request.id))
                } else {
                    let request: HandleRequest = decode(body)?;
                    (request.instance_id, None)
                };
                let status = controller.status(account, definition, id, now_ms)?;
                if operation == "get" {
                    Ok(serde_json::json!({"id":external,"instanceId":id}))
                } else {
                    serde_json::to_value(status)
                        .map_err(|_| failure(ErrorCode::WorkflowInvariantViolation))
                }
            }
            "restart" => {
                let request: RestartRequest = decode(body)?;
                let result = controller.restart(
                    account,
                    definition,
                    request.instance_id,
                    request.operation_id,
                    now_ms,
                );
                if let Some(metrics) = &self.metrics {
                    metrics.workflow_lifecycle("restart", result.is_ok());
                }
                result?;
                Ok(serde_json::json!({"ok":true}))
            }
            "pause" | "resume" | "terminate" => {
                use open_compute_storage::scheduler::WorkflowInstanceAction;
                let request: HandleRequest = decode(body)?;
                let action = match operation {
                    "pause" => WorkflowInstanceAction::Pause,
                    "resume" => WorkflowInstanceAction::Resume,
                    _ => WorkflowInstanceAction::Terminate,
                };
                let result =
                    controller.modify(account, definition, request.instance_id, action, now_ms);
                if let Some(metrics) = &self.metrics {
                    metrics.workflow_lifecycle(operation, result.is_ok());
                }
                result?;
                Ok(serde_json::json!({"ok":true}))
            }
            "send-event" => {
                let request: EventRequest = decode(body)?;
                let result = controller.send_event(
                    account,
                    definition,
                    request.instance_id,
                    &request.event_type,
                    &request.payload_json,
                    now_ms,
                );
                if let Some(metrics) = &self.metrics {
                    metrics.workflow_event(result.as_ref().err().map(PlatformError::code));
                }
                result?;
                Ok(serde_json::json!({"ok":true}))
            }
            _ => Err(failure(ErrorCode::WorkflowMethodUnsupported)),
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RestartRequest {
    instance_id: WorkflowInstanceId,
    operation_id: open_compute_core::WorkflowOperationId,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BatchRequest {
    steps: Vec<WorkflowStepDeclaration>,
    remaining_ms: u64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ResultRequest {
    ordinal: u32,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct YieldRequest {
    final_ordinal: u32,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct OutputRequest {
    output_json: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ErrorRequest {
    code: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct EventRequest {
    instance_id: WorkflowInstanceId,
    #[serde(rename = "type")]
    event_type: String,
    payload_json: String,
}

impl WorkflowBindingService {
    fn run(&self, operation: &str, body: Value, now_ms: i64) -> Result<Value, PlatformError> {
        let (fence, body) = run_fence(body)?;
        let _admission = if operation == "result" {
            None
        } else {
            Some(self.storage.reserve_mutation(match operation {
                "success" => 2 * 1024 * 1024,
                "claim-batch" => 128 * 1024,
                _ => 8192,
            })?)
        };
        match operation {
            "claim-batch" => {
                let request: BatchRequest = decode(body)?;
                if request.steps.is_empty() || request.steps.len() > 16 {
                    return Err(failure(ErrorCode::WorkflowParallelStepUnsupported));
                }
                let descriptors = request
                    .steps
                    .into_iter()
                    .map(WorkflowStepDeclaration::resolve)
                    .collect::<Result<Vec<_>, _>>()?;
                let steps = self.scheduler.claim_workflow_batch(
                    &fence,
                    &descriptors,
                    request.remaining_ms,
                    now_ms,
                    &self.config,
                )?;
                if let Some(metrics) = &self.metrics {
                    for grant in &steps {
                        match grant {
                            WorkflowStepGrant::Complete => metrics.workflow_replay(false),
                            WorkflowStepGrant::Failed => metrics.workflow_replay(true),
                            WorkflowStepGrant::Run { .. } | WorkflowStepGrant::Suspended => {}
                        }
                    }
                }
                Ok(serde_json::json!({"steps":steps}))
            }
            "result" => {
                let request: ResultRequest = decode(body)?;
                serde_json::to_value(self.scheduler.workflow_step_result(
                    &fence,
                    request.ordinal,
                    now_ms,
                )?)
                .map_err(|_| failure(ErrorCode::WorkflowInvariantViolation))
            }
            "register-sleep" | "register-wait" => {
                let declaration: WorkflowStepDeclaration = decode(body)?;
                if (declaration.kind == open_compute_core::workflow::WorkflowStepKind::WaitEvent)
                    != (operation == "register-wait")
                {
                    return Err(failure(ErrorCode::WorkflowStepConfigUnsupported));
                }
                let descriptor = declaration.resolve()?;
                serde_json::to_value(self.scheduler.register_workflow_wait(
                    &fence,
                    &descriptor,
                    now_ms,
                    &self.config,
                )?)
                .map_err(|_| failure(ErrorCode::WorkflowInvariantViolation))
            }
            "yield" => {
                let request: YieldRequest = decode(body)?;
                if request.final_ordinal > 1024 {
                    return Err(failure(ErrorCode::WorkflowStepLimitExceeded));
                }
                self.scheduler.yield_workflow(&fence, now_ms)?;
                Ok(serde_json::json!({"ok":true}))
            }
            "success" | "failure" | "timeout" => {
                let Value::Object(mut fields) = body else {
                    return Err(failure(ErrorCode::WorkflowStepStale));
                };
                let attempt: WorkflowStepAttempt = decode(
                    serde_json::json!({"ordinal":fields.remove("ordinal"),"attempt":fields.remove("attempt"),"stepToken":fields.remove("stepToken")}),
                )?;
                let output;
                let outcome = match operation {
                    "success" => {
                        let request: OutputRequest = decode(Value::Object(fields))?;
                        output = request.output_json;
                        WorkflowStepOutcome::Success(&output)
                    }
                    "failure" => {
                        let request: ErrorRequest = decode(Value::Object(fields))?;
                        WorkflowStepOutcome::Failure(
                            open_compute_core::workflow::terminal_error_code(&request.code)?,
                        )
                    }
                    _ => {
                        if !fields.is_empty() {
                            return Err(failure(ErrorCode::WorkflowMethodUnsupported));
                        }
                        WorkflowStepOutcome::Timeout
                    }
                };
                let started = std::time::Instant::now();
                let result = self.scheduler.settle_workflow_step(
                    &fence,
                    &attempt,
                    outcome,
                    now_ms,
                    &self.config,
                );
                if let Some(metrics) = &self.metrics {
                    metrics.workflow_step(
                        if result.is_ok() {
                            WorkflowOutcome::Success
                        } else {
                            WorkflowOutcome::Error
                        },
                        started.elapsed(),
                    );
                }
                serde_json::to_value(result?)
                    .map_err(|_| failure(ErrorCode::WorkflowInvariantViolation))
            }
            _ => Err(failure(ErrorCode::WorkflowMethodUnsupported)),
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CreateRequest {
    id: Option<String>,
    payload_json: String,
    retention: Option<Value>,
}

fn run_fence(body: Value) -> Result<(WorkflowFence, Value), PlatformError> {
    let Value::Object(mut fields) = body else {
        return Err(failure(ErrorCode::WorkflowRunStale));
    };
    let fence = decode(serde_json::json!({"instanceId":fields.remove("instanceId"),
        "instanceGeneration":fields.remove("instanceGeneration"),"runToken":fields.remove("runToken")}))?;
    Ok((fence, Value::Object(fields)))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InstanceRequest {
    id: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct HandleRequest {
    instance_id: WorkflowInstanceId,
}
fn decode<T: serde::de::DeserializeOwned>(body: Value) -> Result<T, PlatformError> {
    serde_json::from_value(body).map_err(|_| failure(ErrorCode::WorkflowSerializationUnsupported))
}
fn header<'a>(headers: &'a HeaderMap, name: &str) -> Result<&'a str, PlatformError> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| failure(ErrorCode::WorkflowBindingStale))
}
fn failure(code: ErrorCode) -> PlatformError {
    PlatformError::new(code, "Workflow operation failed")
}

pub(crate) fn response_error(code: ErrorCode) -> Response {
    let status = match code {
        ErrorCode::WorkflowRuntimeUnavailable
        | ErrorCode::WorkflowInvariantViolation
        | ErrorCode::StoragePressure
        | ErrorCode::PlatformUnavailable => StatusCode::SERVICE_UNAVAILABLE,
        ErrorCode::WorkflowInstanceAlreadyExists
        | ErrorCode::WorkflowInstanceStateConflict
        | ErrorCode::WorkflowInstanceBusy
        | ErrorCode::WorkflowInstanceCleanupPending
        | ErrorCode::WorkflowRunStale
        | ErrorCode::WorkflowStepStale => StatusCode::CONFLICT,
        ErrorCode::WorkflowNotFound | ErrorCode::WorkflowInstanceNotFound => StatusCode::NOT_FOUND,
        ErrorCode::WorkflowStateQuotaExceeded
        | ErrorCode::WorkflowStepLimitExceeded
        | ErrorCode::WorkflowEventQueueFull => StatusCode::TOO_MANY_REQUESTS,
        ErrorCode::WorkflowPayloadTooLarge | ErrorCode::WorkflowResultTooLarge => {
            StatusCode::PAYLOAD_TOO_LARGE
        }
        _ => StatusCode::UNPROCESSABLE_ENTITY,
    };
    let mut response = status.into_response();
    response.headers_mut().insert(
        HeaderName::from_static("x-open-compute-error-code"),
        HeaderValue::from_static(code.as_str()),
    );
    response
}

#[cfg(test)]
#[path = "workflow_backend_tests.rs"]
mod tests;
