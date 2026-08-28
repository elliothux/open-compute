//! Strict private V2 protocol mapping; workflows remain owned by storage and workers.

use super::*;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct RestartRequest {
    pub instance_id: WorkflowInstanceId,
    pub operation_id: open_compute_core::WorkflowOperationId,
}
use open_compute_core::workflow::WorkflowStepDeclaration;
use open_compute_storage::scheduler::{WorkflowStepAttempt, WorkflowStepOutcome};

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
pub(super) struct EventRequest {
    pub instance_id: WorkflowInstanceId,
    #[serde(rename = "type")]
    pub event_type: String,
    pub payload_json: String,
}

impl WorkflowBindingService {
    pub(super) fn run_v2(
        &self,
        operation: &str,
        body: Value,
        now_ms: i64,
    ) -> Result<Value, PlatformError> {
        let (fence, body) = run_fence(body)?;
        let _admission = if operation == "result" {
            None
        } else {
            Some(self.storage.reserve_mutation(
                OperationClass::Scheduler,
                match operation {
                    "success" => 2 * 1024 * 1024,
                    "claim-batch" => 128 * 1024,
                    _ => 8192,
                },
            )?)
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
                Ok(
                    serde_json::json!({"steps":self.scheduler.claim_workflow_batch_v2(&fence,&descriptors,request.remaining_ms,now_ms,&self.config)?}),
                )
            }
            "result" => {
                let request: ResultRequest = decode(body)?;
                serde_json::to_value(self.scheduler.workflow_step_result_v2(
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
                serde_json::to_value(self.scheduler.register_workflow_wait_v2(
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
                self.scheduler.yield_workflow_v2(&fence, now_ms)?;
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
                            open_compute_core::workflow::terminal_error_code_v2(&request.code)?,
                        )
                    }
                    _ => {
                        if !fields.is_empty() {
                            return Err(failure(ErrorCode::WorkflowMethodUnsupported));
                        }
                        WorkflowStepOutcome::Timeout
                    }
                };
                serde_json::to_value(self.scheduler.settle_workflow_step_v2(
                    &fence,
                    &attempt,
                    outcome,
                    now_ms,
                    &self.config,
                )?)
                .map_err(|_| failure(ErrorCode::WorkflowInvariantViolation))
            }
            _ => Err(failure(ErrorCode::WorkflowMethodUnsupported)),
        }
    }
}
