//! Authenticated Workflow caller methods and private run/step persistence protocol.

use crate::metrics::{MetricsRegistry, WorkflowOutcome};
use axum::body::to_bytes;
use axum::extract::Request;
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode};
use axum::response::{IntoResponse as _, Response};
use open_compute_core::workflow::WorkflowStepDeclaration;
use open_compute_core::{
    BindingId, DeploymentId, ErrorCode, PlatformError, SchedulerClock as _, WorkflowFence,
    WorkflowInstanceId, WorkflowOperationId, WorkflowsConfig,
};
use open_compute_runtime::GenerationAuthRegistry;
use open_compute_storage::scheduler::{
    WorkflowStepAttempt, WorkflowStepGrant, WorkflowStepOutcome,
};
use open_compute_storage::{PlatformStorage, SchedulerStore, WorkflowRepository};
use open_compute_workers::{WorkflowController, WorkflowEventInput};
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest as _, Sha256};
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
        let mutation = !matches!(operation, "get" | "status");
        let operation_id = mutation
            .then(|| {
                header(headers, "x-open-compute-request-id")?
                    .parse()
                    .map_err(|_| failure(ErrorCode::WorkflowMethodUnsupported))
            })
            .transpose()?;
        let request_json = serde_json::to_vec(&body)
            .map_err(|_| failure(ErrorCode::WorkflowSerializationUnsupported))?;
        if mutation {
            let fingerprint = workflow_binding_operation_fingerprint(
                binding.descriptor.binding_id,
                operation,
                &request_json,
            );
            if let Some(replay) = repository.begin_binding_operation(
                binding.descriptor.binding_id,
                operation_id.ok_or_else(|| failure(ErrorCode::WorkflowInvariantViolation))?,
                operation,
                &fingerprint,
                &request_json,
                now_ms,
            )? {
                let replay: Value = serde_json::from_slice(&replay)
                    .map_err(|_| failure(ErrorCode::WorkflowInvariantViolation))?;
                if let Some(code) = replay.get("errorCode").and_then(Value::as_str) {
                    return Err(failure(workflow_error_code(code)?));
                }
                return Ok(replay);
            }
        }
        let result = (|| -> Result<Value, PlatformError> {
            match operation {
                "create" => {
                    let request: CreateRequest = decode(body)?;
                    validate_location(request.location_hint.as_deref())?;
                    if let Some(schedule) = &request.schedule {
                        schedule.validate()?;
                        if !binding.descriptor.schedules.contains(&schedule.cron) {
                            return Err(failure(ErrorCode::WorkflowMethodUnsupported));
                        }
                    }
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
                        operation_id
                            .ok_or_else(|| failure(ErrorCode::WorkflowInvariantViolation))?,
                        request.id.as_deref(),
                        open_compute_workers::WorkflowCreateInput {
                            payload_base64: &request.payload_base64,
                            retention: retention.as_ref(),
                            schedule: request.schedule.as_ref(),
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
                "create-batch" => {
                    let request: CreateBatchRequest = decode(body)?;
                    if request.instances.is_empty() || request.instances.len() > 100 {
                        return Err(failure(ErrorCode::WorkflowMethodUnsupported));
                    }
                    let batch_operation_id = operation_id
                        .ok_or_else(|| failure(ErrorCode::WorkflowInvariantViolation))?;
                    let mut prepared = Vec::with_capacity(request.instances.len());
                    for (ordinal, request) in request.instances.into_iter().enumerate() {
                        if request.schedule.is_some() {
                            return Err(failure(ErrorCode::WorkflowMethodUnsupported));
                        }
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
                        validate_location(request.location_hint.as_deref())?;
                        prepared.push((
                            workflow_batch_item_operation_id(batch_operation_id, ordinal)?,
                            request.id,
                            request.payload_base64,
                            retention,
                        ));
                    }
                    let create_requests = prepared
                        .iter()
                        .map(|(operation, external, payload, retention)| {
                            (
                                *operation,
                                external.as_deref(),
                                open_compute_workers::WorkflowCreateInput {
                                    payload_base64: payload,
                                    retention: retention.as_ref(),
                                    schedule: None,
                                },
                            )
                        })
                        .collect::<Vec<_>>();
                    let instances = controller
                        .create_batch(
                            account,
                            definition,
                            batch_operation_id,
                            &create_requests,
                            now_ms,
                        )?
                        .into_iter()
                        .map(|identity| serde_json::json!({"id":identity.external_instance_id,"instanceId":identity.instance_id}))
                        .collect::<Vec<_>>();
                    Ok(serde_json::json!({"instances":instances}))
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
                        operation_id
                            .ok_or_else(|| failure(ErrorCode::WorkflowInvariantViolation))?,
                        request.from,
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
                    let request: ModifyRequest = decode(body)?;
                    if operation != "terminate" && request.rollback.is_some() {
                        return Err(failure(ErrorCode::WorkflowMethodUnsupported));
                    }
                    let action = match operation {
                        "pause" => WorkflowInstanceAction::Pause,
                        "resume" => WorkflowInstanceAction::Resume,
                        _ => WorkflowInstanceAction::Terminate,
                    };
                    let result = if operation == "terminate" && request.rollback.unwrap_or(false) {
                        controller.rollback(account, definition, request.instance_id, now_ms)
                    } else {
                        controller.modify(account, definition, request.instance_id, action, now_ms)
                    };
                    if let Some(metrics) = &self.metrics {
                        metrics.workflow_lifecycle(operation, result.is_ok());
                    }
                    result?;
                    Ok(serde_json::json!({"ok":true}))
                }
                "delete" => {
                    let request: HandleRequest = decode(body)?;
                    controller.delete(
                        account,
                        definition,
                        request.instance_id,
                        operation_id
                            .ok_or_else(|| failure(ErrorCode::WorkflowInvariantViolation))?,
                        now_ms,
                    )?;
                    Ok(serde_json::json!({"ok":true}))
                }
                "delete-batch" => {
                    let request: DeleteBatchRequest = decode(body)?;
                    if request.instance_ids.is_empty() || request.instance_ids.len() > 100 {
                        return Err(failure(ErrorCode::WorkflowMethodUnsupported));
                    }
                    let batch_operation_id = operation_id
                        .ok_or_else(|| failure(ErrorCode::WorkflowInvariantViolation))?;
                    let mut decisions = std::collections::HashMap::<String, bool>::new();
                    let mut deleted = Vec::new();
                    let mut errors = Vec::new();
                    for id in request.instance_ids {
                        let success = if let Some(success) = decisions.get(&id) {
                            *success
                        } else {
                            let success = repository
                                .find_instance(definition, &id)
                                .and_then(|reservation| {
                                    controller.delete(
                                        account,
                                        definition,
                                        reservation.identity.instance_id,
                                        workflow_named_item_operation_id(batch_operation_id, &id)?,
                                        now_ms,
                                    )
                                })
                                .is_ok();
                            decisions.insert(id.clone(), success);
                            success
                        };
                        if success {
                            deleted.push(serde_json::json!({"id":id}));
                        } else {
                            errors.push(serde_json::json!({"id":id,"code":404,"message":"Workflow instance not found"}));
                        }
                    }
                    Ok(serde_json::json!({"deleted":deleted,"errors":errors}))
                }
                "send-event" => {
                    let request: EventRequest = decode(body)?;
                    let result = controller.send_event(
                        account,
                        definition,
                        request.instance_id,
                        WorkflowEventInput {
                            operation_id: operation_id
                                .ok_or_else(|| failure(ErrorCode::WorkflowInvariantViolation))?,
                            event_type: &request.event_type,
                            payload_base64: &request.payload_base64,
                        },
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
        })();
        if mutation {
            let response = match &result {
                Ok(response) => response.clone(),
                Err(error) => serde_json::json!({"errorCode":error.code().as_str()}),
            };
            let encoded = serde_json::to_vec(&response)
                .map_err(|_| failure(ErrorCode::WorkflowInvariantViolation))?;
            repository.finish_binding_operation(
                binding.descriptor.binding_id,
                operation_id.ok_or_else(|| failure(ErrorCode::WorkflowInvariantViolation))?,
                &encoded,
                now_ms,
            )?;
        }
        result
    }
}

fn workflow_binding_operation_fingerprint(
    binding: BindingId,
    operation: &str,
    request_json: &[u8],
) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(binding.to_string());
    hasher.update([0]);
    hasher.update(operation);
    hasher.update([0]);
    hasher.update(request_json);
    hasher.finalize().into()
}

fn workflow_batch_item_operation_id(
    batch_operation_id: WorkflowOperationId,
    ordinal: usize,
) -> Result<WorkflowOperationId, PlatformError> {
    let mut hasher = Sha256::new();
    hasher.update(batch_operation_id.as_uuid().as_bytes());
    hasher.update([0]);
    hasher.update(
        u64::try_from(ordinal)
            .map_err(|_| failure(ErrorCode::WorkflowInvariantViolation))?
            .to_be_bytes(),
    );
    workflow_operation_id_from_digest(hasher.finalize().into())
}

fn workflow_named_item_operation_id(
    batch_operation_id: WorkflowOperationId,
    name: &str,
) -> Result<WorkflowOperationId, PlatformError> {
    let mut hasher = Sha256::new();
    hasher.update(batch_operation_id.as_uuid().as_bytes());
    hasher.update([0]);
    hasher.update(name.as_bytes());
    workflow_operation_id_from_digest(hasher.finalize().into())
}

fn workflow_operation_id_from_digest(
    digest: [u8; 32],
) -> Result<WorkflowOperationId, PlatformError> {
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    WorkflowOperationId::from_uuid(uuid::Uuid::from_bytes(bytes))
        .map_err(|_| failure(ErrorCode::WorkflowInvariantViolation))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RestartRequest {
    instance_id: WorkflowInstanceId,
    from: Option<open_compute_core::workflow::WorkflowRestartSelector>,
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
    output_base64: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ErrorRequest {
    code: String,
    #[serde(rename = "resolvedDelayMs")]
    resolved_delay_ms: Option<u64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ResolveDelayRequest {
    ordinal: u32,
    attempt: u32,
    code: String,
    resolved_delay_ms: Option<u64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct EventRequest {
    instance_id: WorkflowInstanceId,
    #[serde(rename = "type")]
    event_type: String,
    payload_base64: String,
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
                    return Err(failure(ErrorCode::WorkflowStepLimitExceeded));
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
                            WorkflowStepGrant::Complete { .. } => metrics.workflow_replay(false),
                            WorkflowStepGrant::Failed => metrics.workflow_replay(true),
                            WorkflowStepGrant::Run { .. }
                            | WorkflowStepGrant::ResolveDelay { .. }
                            | WorkflowStepGrant::RollbackBoundary { .. }
                            | WorkflowStepGrant::Suspended => {}
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
            "yield" => {
                let request: YieldRequest = decode(body)?;
                if request.final_ordinal > 1024 {
                    return Err(failure(ErrorCode::WorkflowStepLimitExceeded));
                }
                self.scheduler.yield_workflow(&fence, now_ms)?;
                Ok(serde_json::json!({"ok":true}))
            }
            "resolve-delay" => {
                let request: ResolveDelayRequest = decode(body)?;
                serde_json::to_value(self.scheduler.resolve_workflow_delay(
                    &fence,
                    request.ordinal,
                    request.attempt,
                    open_compute_storage::scheduler::WorkflowDelayResolution {
                        failure_code: open_compute_core::workflow::terminal_error_code(
                            &request.code,
                        )?,
                        resolved_delay_ms: request.resolved_delay_ms,
                    },
                    now_ms,
                    &self.config,
                )?)
                .map_err(|_| failure(ErrorCode::WorkflowInvariantViolation))
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
                        output = request.output_base64;
                        WorkflowStepOutcome::Success(&output)
                    }
                    "failure" => {
                        let request: ErrorRequest = decode(Value::Object(fields))?;
                        let code = open_compute_core::workflow::terminal_error_code(&request.code)?;
                        match request.resolved_delay_ms {
                            Some(delay) => WorkflowStepOutcome::FailureWithDelay(code, delay),
                            None => WorkflowStepOutcome::Failure(code),
                        }
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
    payload_base64: String,
    retention: Option<Value>,
    location_hint: Option<String>,
    schedule: Option<open_compute_core::WorkflowCronSchedule>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateBatchRequest {
    instances: Vec<CreateRequest>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DeleteBatchRequest {
    instance_ids: Vec<String>,
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

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ModifyRequest {
    instance_id: WorkflowInstanceId,
    rollback: Option<bool>,
}

fn validate_location(value: Option<&str>) -> Result<(), PlatformError> {
    if value.is_some_and(|value| {
        !matches!(
            value,
            "wnam"
                | "enam"
                | "sam"
                | "weur"
                | "eeur"
                | "apac"
                | "apac-ne"
                | "apac-se"
                | "oc"
                | "afr"
                | "me"
        )
    }) {
        return Err(failure(ErrorCode::WorkflowMethodUnsupported));
    }
    Ok(())
}

fn workflow_error_code(value: &str) -> Result<ErrorCode, PlatformError> {
    [
        ErrorCode::WorkflowRuntimeUnavailable,
        ErrorCode::WorkflowInvariantViolation,
        ErrorCode::WorkflowInstanceAlreadyExists,
        ErrorCode::WorkflowInstanceStateConflict,
        ErrorCode::WorkflowInstanceBusy,
        ErrorCode::WorkflowInstanceCleanupPending,
        ErrorCode::WorkflowInstanceNotFound,
        ErrorCode::WorkflowStateQuotaExceeded,
        ErrorCode::WorkflowPayloadTooLarge,
        ErrorCode::WorkflowResultTooLarge,
        ErrorCode::WorkflowMethodUnsupported,
        ErrorCode::WorkflowSerializationUnsupported,
    ]
    .into_iter()
    .find(|code| code.as_str() == value)
    .ok_or_else(|| failure(ErrorCode::WorkflowInvariantViolation))
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
