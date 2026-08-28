//! Exact attempt commits, deterministic retry decisions and immutable replay verdicts.

use super::durable_model::{DurableStep, read_step};
use super::*;
use open_compute_core::workflow::WorkflowDurableConfig;

impl SchedulerStore {
    /// Commit one trusted callback report under the exact run, ordinal, attempt and step token.
    /// The stored deadline wins over a late success or failure, independently of host timer order.
    pub fn settle_workflow_step_v2(
        &self,
        fence: &WorkflowFence,
        attempt: &WorkflowStepAttempt,
        outcome: WorkflowStepOutcome<'_>,
        now_ms: i64,
        limits: &WorkflowsConfig,
    ) -> Result<WorkflowV2StepResult, PlatformError> {
        limits.validate()?;
        let mut conn = self.lock()?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sql_error)?;
        let instance = running(&tx, fence, now_ms)?;
        if instance.durable.is_none() {
            return Err(error(ErrorCode::WorkflowCapabilityMismatch));
        }
        let step = read_step(
            &tx,
            fence.instance_id,
            fence.instance_generation,
            attempt.ordinal,
        )?
        .ok_or_else(|| error(ErrorCode::WorkflowStepStale))?;
        if step.state != "running"
            || step.attempt != attempt.attempt
            || step.run_token.as_ref() != Some(&fence.run_token)
            || step.step_token.as_ref() != Some(&attempt.step_token)
        {
            return Err(error(ErrorCode::WorkflowStepStale));
        }
        let expired = step
            .deadline
            .ok_or_else(|| error(ErrorCode::WorkflowInvariantViolation))?
            <= now_ms;
        let verdict = if expired {
            fail(
                &tx,
                fence.instance_id,
                fence.instance_generation,
                &step,
                ErrorCode::WorkflowStepTimeout,
                now_ms,
            )?
        } else {
            match outcome {
                WorkflowStepOutcome::Timeout => return Err(error(ErrorCode::WorkflowStepStale)),
                WorkflowStepOutcome::Failure(code) => fail(
                    &tx,
                    fence.instance_id,
                    fence.instance_generation,
                    &step,
                    code,
                    now_ms,
                )?,
                WorkflowStepOutcome::Success(output) => {
                    match open_compute_core::workflow::canonical_json(
                        output,
                        ErrorCode::WorkflowResultTooLarge,
                    )
                    .and_then(|value| {
                        capacity_v2(&tx, &instance, value.len() as i64, -1, limits)?;
                        Ok(value)
                    }) {
                        Ok(output) => {
                            tx.execute("UPDATE workflow_steps SET state='complete',run_token=NULL,step_token=NULL,output_json=?4,
                                completed_at_ms=?5,updated_at_ms=?5 WHERE instance_id=?1 AND instance_generation=?2 AND ordinal=?3",
                                params![fence.instance_id.to_string(),fence.instance_generation,attempt.ordinal,output.as_bytes(),now_ms]).map_err(sql_error)?;
                            WorkflowV2StepResult::Complete {
                                output_json: Some(output),
                            }
                        }
                        Err(err) => fail(
                            &tx,
                            fence.instance_id,
                            fence.instance_generation,
                            &step,
                            err.code(),
                            now_ms,
                        )?,
                    }
                }
            }
        };
        if matches!(verdict, WorkflowV2StepResult::Suspended) {
            durable_steps::request_yield(&tx, fence, now_ms)?;
        }
        heartbeat(&tx, fence, now_ms, limits)?;
        tx.commit().map_err(sql_error)?;
        Ok(verdict)
    }
}

/// Failure bytes consume the reservation established before callback/event admission.
/// The caller requests yield only after it has finished granting any ready siblings.
pub(super) fn fail(
    conn: &Connection,
    id: WorkflowInstanceId,
    generation: i64,
    step: &DurableStep,
    code: ErrorCode,
    now_ms: i64,
) -> Result<WorkflowV2StepResult, PlatformError> {
    open_compute_core::workflow::terminal_error_code_v2(code.as_str())?;
    let WorkflowDurableConfig::Do(config) = &step.descriptor.config else {
        return Err(error(ErrorCode::WorkflowInvariantViolation));
    };
    if step.attempt == 0 || !matches!(step.state.as_str(), "running" | "pending") {
        return Err(error(ErrorCode::WorkflowStepStale));
    }
    let retry = matches!(
        code,
        ErrorCode::WorkflowExecutionFailed | ErrorCode::WorkflowStepTimeout
    ) && step.attempt <= config.retries.limit;
    let code = if !retry && code == ErrorCode::WorkflowExecutionFailed {
        ErrorCode::WorkflowStepRetriesExhausted
    } else {
        code
    };
    let due = if retry {
        Some(durable_deadline(
            now_ms,
            config.retries.delay_after(step.attempt)?,
        )?)
    } else {
        None
    };
    conn.execute("UPDATE workflow_steps SET state=?4,run_token=NULL,step_token=NULL,error_json=?5,error_code=?6,
        due_at_ms=?7,updated_at_ms=?8,completed_at_ms=?9 WHERE instance_id=?1 AND instance_generation=?2 AND ordinal=?3",
        params![id.to_string(),generation,step.descriptor.ordinal,if retry {"retry_wait"} else {"failed"},
            failure_json().as_bytes(),code.as_str(),due,now_ms,if retry {None} else {Some(now_ms)}]).map_err(sql_error)?;
    Ok(if retry {
        WorkflowV2StepResult::Suspended
    } else {
        WorkflowV2StepResult::Failed {
            code: code.as_str().into(),
        }
    })
}
