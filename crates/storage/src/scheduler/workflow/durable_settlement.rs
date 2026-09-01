//! Exact attempt commits, deterministic retry decisions and immutable replay verdicts.

use super::durable_model::{DurableStep, read_step};
use super::*;
use open_compute_core::workflow::WorkflowDurableConfig;

impl SchedulerStore {
    /// Commit one trusted callback report under the exact run, ordinal, attempt and step token.
    /// The stored deadline wins over a late success or failure, independently of host timer order.
    pub fn settle_workflow_step(
        &self,
        fence: &WorkflowFence,
        attempt: &WorkflowStepAttempt,
        outcome: WorkflowStepOutcome<'_>,
        now_ms: i64,
        limits: &WorkflowsConfig,
    ) -> Result<WorkflowStepResult, PlatformError> {
        limits.validate()?;
        let mut conn = self.lock()?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sql_error)?;
        let instance = running(&tx, fence, now_ms)?;
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
            if matches!(
                &step.descriptor.config,
                WorkflowDurableConfig::Do(config) if config.retries.delay.is_none()
            ) {
                defer_dynamic_failure(
                    &tx,
                    fence.instance_id,
                    fence.instance_generation,
                    &step,
                    ErrorCode::WorkflowStepTimeout,
                    now_ms,
                )?
            } else {
                fail(
                    &tx,
                    fence.instance_id,
                    fence.instance_generation,
                    &step,
                    ErrorCode::WorkflowStepTimeout,
                    None,
                    now_ms,
                )?
            }
        } else {
            match outcome {
                WorkflowStepOutcome::Timeout => return Err(error(ErrorCode::WorkflowStepStale)),
                WorkflowStepOutcome::Failure(code) => fail(
                    &tx,
                    fence.instance_id,
                    fence.instance_generation,
                    &step,
                    code,
                    None,
                    now_ms,
                )?,
                WorkflowStepOutcome::FailureWithDelay(code, delay_ms) => fail(
                    &tx,
                    fence.instance_id,
                    fence.instance_generation,
                    &step,
                    code,
                    Some(delay_ms),
                    now_ms,
                )?,
                WorkflowStepOutcome::Success(output) => {
                    match open_compute_core::workflow::durable_value_base64(
                        output,
                        ErrorCode::WorkflowResultTooLarge,
                    )
                    .and_then(|value| {
                        capacity_change(&tx, &instance, value.len() as i64, -1, limits)?;
                        Ok(value)
                    }) {
                        Ok(output) => {
                            tx.execute("UPDATE workflow_steps SET state='complete',run_token=NULL,step_token=NULL,output_json=?4,
                                completed_at_ms=?5,updated_at_ms=?5 WHERE instance_id=?1 AND instance_generation=?2 AND ordinal=?3",
                                params![fence.instance_id.to_string(),fence.instance_generation,attempt.ordinal,output.as_bytes(),now_ms]).map_err(sql_error)?;
                            WorkflowStepResult::Complete {
                                output_base64: Some(output),
                            }
                        }
                        Err(err) => fail(
                            &tx,
                            fence.instance_id,
                            fence.instance_generation,
                            &step,
                            err.code(),
                            None,
                            now_ms,
                        )?,
                    }
                }
            }
        };
        if matches!(verdict, WorkflowStepResult::Suspended) {
            durable_steps::request_yield(&tx, fence, now_ms)?;
        }
        heartbeat(&tx, fence, now_ms, limits)?;
        tx.commit().map_err(sql_error)?;
        Ok(verdict)
    }

    /// Resolve a previously persisted dynamic-delay timeout without rerunning
    /// the business callback or trusting process memory as retry authority.
    pub fn resolve_workflow_delay(
        &self,
        fence: &WorkflowFence,
        ordinal: u32,
        attempt: u32,
        resolution: WorkflowDelayResolution,
        now_ms: i64,
        limits: &WorkflowsConfig,
    ) -> Result<WorkflowStepResult, PlatformError> {
        let WorkflowDelayResolution {
            failure_code,
            resolved_delay_ms,
        } = resolution;
        limits.validate()?;
        let mut conn = self.lock()?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sql_error)?;
        let _instance = running(&tx, fence, now_ms)?;
        let step = read_step(&tx, fence.instance_id, fence.instance_generation, ordinal)?
            .ok_or_else(|| error(ErrorCode::WorkflowStepStale))?;
        let WorkflowDurableConfig::Do(config) = &step.descriptor.config else {
            return Err(error(ErrorCode::WorkflowStepStale));
        };
        if step.state != "delay_pending"
            || step.attempt != attempt
            || config.retries.delay.is_some()
            || step.failure.as_deref() != Some(ErrorCode::WorkflowStepTimeout.as_str())
        {
            return Err(error(ErrorCode::WorkflowStepStale));
        }
        let verdict = if failure_code == ErrorCode::WorkflowStepConfigUnsupported {
            fail(
                &tx,
                fence.instance_id,
                fence.instance_generation,
                &step,
                failure_code,
                None,
                now_ms,
            )?
        } else {
            if failure_code != ErrorCode::WorkflowStepTimeout {
                return Err(error(ErrorCode::WorkflowStepStale));
            }
            fail(
                &tx,
                fence.instance_id,
                fence.instance_generation,
                &step,
                failure_code,
                resolved_delay_ms,
                now_ms,
            )?
        };
        if matches!(verdict, WorkflowStepResult::Suspended) {
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
    resolved_dynamic_delay: Option<u64>,
    now_ms: i64,
) -> Result<WorkflowStepResult, PlatformError> {
    open_compute_core::workflow::terminal_error_code(code.as_str())?;
    let WorkflowDurableConfig::Do(config) = &step.descriptor.config else {
        return Err(error(ErrorCode::WorkflowInvariantViolation));
    };
    if step.attempt == 0 || !matches!(step.state.as_str(), "running" | "pending" | "delay_pending")
    {
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
    let retry_delay_ms = if retry {
        Some(
            config
                .retries
                .delay_after_resolved(step.attempt, resolved_dynamic_delay)?,
        )
    } else {
        None
    };
    let due = retry_delay_ms
        .map(|delay| durable_deadline(now_ms, delay))
        .transpose()?;
    conn.execute("UPDATE workflow_steps SET state=?4,run_token=NULL,step_token=NULL,error_json=?5,error_code=?6,
        due_at_ms=?7,retry_delay_ms=?8,updated_at_ms=?9,completed_at_ms=?10 WHERE instance_id=?1 AND instance_generation=?2 AND ordinal=?3",
        params![id.to_string(),generation,step.descriptor.ordinal,if retry {"retry_wait"} else {"failed"},
            failure_json().as_bytes(),code.as_str(),due,retry_delay_ms,now_ms,if retry {None} else {Some(now_ms)}]).map_err(sql_error)?;
    Ok(if retry {
        WorkflowStepResult::Suspended
    } else {
        WorkflowStepResult::Failed {
            code: code.as_str().into(),
        }
    })
}

pub(super) fn defer_dynamic_failure(
    conn: &Connection,
    id: WorkflowInstanceId,
    generation: i64,
    step: &DurableStep,
    code: ErrorCode,
    now_ms: i64,
) -> Result<WorkflowStepResult, PlatformError> {
    let WorkflowDurableConfig::Do(config) = &step.descriptor.config else {
        return Err(error(ErrorCode::WorkflowInvariantViolation));
    };
    if config.retries.delay.is_some()
        || step.attempt == 0
        || !matches!(step.state.as_str(), "running" | "pending")
        || code != ErrorCode::WorkflowStepTimeout
    {
        return Err(error(ErrorCode::WorkflowStepStale));
    }
    conn.execute(
        "UPDATE workflow_steps SET state='delay_pending',run_token=NULL,step_token=NULL,error_json=?4,error_code=?5,
         due_at_ms=NULL,retry_delay_ms=NULL,updated_at_ms=?6 WHERE instance_id=?1 AND instance_generation=?2 AND ordinal=?3",
        params![id.to_string(),generation,step.descriptor.ordinal,failure_json().as_bytes(),code.as_str(),now_ms],
    ).map_err(sql_error)?;
    Ok(WorkflowStepResult::ResolveDelay {
        attempt: step.attempt,
        code: code.as_str().into(),
        config: config.clone(),
    })
}

pub(super) fn timeout(
    conn: &Connection,
    id: WorkflowInstanceId,
    generation: i64,
    step: &DurableStep,
    now_ms: i64,
) -> Result<WorkflowStepResult, PlatformError> {
    if matches!(
        &step.descriptor.config,
        WorkflowDurableConfig::Do(config) if config.retries.delay.is_none()
    ) {
        defer_dynamic_failure(
            conn,
            id,
            generation,
            step,
            ErrorCode::WorkflowStepTimeout,
            now_ms,
        )
    } else {
        fail(
            conn,
            id,
            generation,
            step,
            ErrorCode::WorkflowStepTimeout,
            None,
            now_ms,
        )
    }
}
