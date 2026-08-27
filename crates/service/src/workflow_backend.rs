//! Authenticated Workflow caller methods and private run/step persistence protocol.

use crate::metrics::{MetricsRegistry, WorkflowOutcome};
use axum::body::to_bytes;
use axum::extract::Request;
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode};
use axum::response::{IntoResponse as _, Response};
use open_compute_core::{
    BindingId, DeploymentId, ErrorCode, OperationClass, PlatformError, SchedulerClock as _,
    WorkflowFence, WorkflowToken, WorkflowsConfig,
};
use open_compute_runtime::GenerationAuthRegistry;
use open_compute_storage::scheduler::{WorkflowFailure, WorkflowStepIdentity};
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
        if let Some(operation) = path.strip_prefix("/internal/workflows/v1/runs/") {
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
        let controller = WorkflowController::new(&self.storage, &self.scheduler, &self.config);
        match operation {
            "create" => {
                if header(headers, "x-open-compute-workflow-do-context")? != "0" {
                    return Err(failure(ErrorCode::WorkflowDoOutputGateUnsupported));
                }
                let request: CreateRequest = decode(body)?;
                let result = controller.create(
                    account,
                    definition,
                    request.id.as_deref(),
                    &request.payload_json,
                    now_ms,
                );
                if let Some(metrics) = &self.metrics {
                    metrics.workflow_created(if result.is_ok() {
                        WorkflowOutcome::Success
                    } else {
                        WorkflowOutcome::Error
                    });
                }
                let id = result?;
                Ok(serde_json::json!({"id":id}))
            }
            "get" | "status" => {
                let request: InstanceRequest = decode(body)?;
                let status = controller.status(account, definition, &request.id)?;
                if operation == "get" {
                    Ok(serde_json::json!({"id":request.id}))
                } else {
                    serde_json::to_value(status)
                        .map_err(|_| failure(ErrorCode::WorkflowInvariantViolation))
                }
            }
            _ => Err(failure(ErrorCode::WorkflowMethodUnsupported)),
        }
    }

    fn run(&self, operation: &str, body: Value, now_ms: i64) -> Result<Value, PlatformError> {
        let Value::Object(mut fields) = body else {
            return Err(failure(ErrorCode::WorkflowRunStale));
        };
        let fence: WorkflowFence = decode(
            serde_json::json!({"instanceId":fields.remove("instanceId"),
            "instanceGeneration":fields.remove("instanceGeneration"),"runToken":fields.remove("runToken")}),
        )?;
        let body = Value::Object(fields);
        // Growth is admitted before the transaction, while terminal cleanup uses its reserved error budget.
        let _admission = self.storage.reserve_mutation(
            OperationClass::Scheduler,
            match operation {
                "success" => 2 * 1024 * 1024,
                "claim" => 64 * 1024,
                _ => 8192,
            },
        )?;
        let result = match operation {
            "claim" => {
                let identity: WorkflowStepIdentity = decode(body)?;
                self.scheduler
                    .claim_workflow_step(&fence, &identity, now_ms, &self.config)
                    .and_then(|grant| {
                        if let Some(metrics) = &self.metrics {
                            match &grant {
                                open_compute_storage::scheduler::WorkflowStepGrant::Complete {
                                    ..
                                } => metrics.workflow_replay(false),
                                open_compute_storage::scheduler::WorkflowStepGrant::Failed {
                                    ..
                                } => metrics.workflow_replay(true),
                                _ => {}
                            }
                        }
                        serde_json::to_value(grant)
                            .map_err(|_| failure(ErrorCode::WorkflowInvariantViolation))
                    })
            }
            "success" => {
                let request: SuccessRequest = decode(body)?;
                let started = self
                    .scheduler
                    .workflow_step_started_at(fence.instance_id, request.ordinal)?;
                self.scheduler
                    .complete_workflow_step(
                        &fence,
                        request.ordinal,
                        &request.step_token,
                        &request.output_json,
                        now_ms,
                        &self.config,
                    )
                    .map(|()| {
                        self.observe_step(WorkflowOutcome::Success, started, now_ms);
                        serde_json::json!({"ok":true})
                    })
            }
            "failure" => {
                let request: FailureRequest = decode(body)?;
                let started = self
                    .scheduler
                    .workflow_step_started_at(fence.instance_id, request.ordinal)?;
                if request.error != WorkflowFailure::default() {
                    return Err(failure(ErrorCode::WorkflowSerializationUnsupported));
                }
                let code = request.error_code.as_deref().map_or(
                    Ok(ErrorCode::WorkflowExecutionFailed),
                    open_compute_core::workflow::terminal_error_code,
                )?;
                self.scheduler
                    .fail_workflow_step(
                        &fence,
                        request.ordinal,
                        &request.step_token,
                        code,
                        now_ms,
                        &self.config,
                    )
                    .map(|()| {
                        self.observe_step(WorkflowOutcome::Error, started, now_ms);
                        serde_json::json!({"ok":true})
                    })
            }
            _ => Err(failure(ErrorCode::WorkflowMethodUnsupported)),
        };
        if let (Some(metrics), Err(error)) = (&self.metrics, &result) {
            match error.code() {
                ErrorCode::WorkflowRunStale => metrics.workflow_stale(false),
                ErrorCode::WorkflowStepStale => metrics.workflow_stale(true),
                _ => {}
            }
        }
        if let Some(instance) = self.scheduler.workflow_instance(fence.instance_id)?
            && instance.state.is_terminal()
        {
            WorkflowRepository::new(self.storage.db())
                .release_instance(&instance.identity, now_ms)?;
        }
        result
    }

    fn observe_step(&self, outcome: WorkflowOutcome, started: Option<i64>, now_ms: i64) {
        if let (Some(metrics), Some(started)) = (&self.metrics, started) {
            metrics.workflow_step(
                outcome,
                Duration::from_millis(now_ms.saturating_sub(started).max(0) as u64),
            );
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CreateRequest {
    id: Option<String>,
    payload_json: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InstanceRequest {
    id: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SuccessRequest {
    ordinal: u32,
    step_token: WorkflowToken,
    output_json: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FailureRequest {
    ordinal: u32,
    step_token: WorkflowToken,
    error: WorkflowFailure,
    error_code: Option<String>,
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
        | ErrorCode::WorkflowRunStale
        | ErrorCode::WorkflowStepStale => StatusCode::CONFLICT,
        ErrorCode::WorkflowNotFound | ErrorCode::WorkflowInstanceNotFound => StatusCode::NOT_FOUND,
        ErrorCode::WorkflowStateQuotaExceeded | ErrorCode::WorkflowStepLimitExceeded => {
            StatusCode::TOO_MANY_REQUESTS
        }
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
