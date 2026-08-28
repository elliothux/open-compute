//! Atomic batch registration, replay comparison and bounded callback admission.

use super::durable_model::{DurableStep, read_step};
use super::*;
use open_compute_core::workflow::{
    WorkflowDurableConfig, WorkflowStepDescriptor, WorkflowStepKind,
};

impl SchedulerStore {
    /// Register an entire immutable batch before granting any callback, or replay its exact shape.
    /// A recovered pending attempt keeps its old attempt number and absolute deadline.
    pub fn claim_workflow_batch_v2(
        &self,
        fence: &WorkflowFence,
        descriptors: &[WorkflowStepDescriptor],
        remaining_ms: u64,
        now_ms: i64,
        limits: &WorkflowsConfig,
    ) -> Result<Vec<WorkflowV2StepGrant>, PlatformError> {
        limits.validate()?;
        let first = descriptors
            .first()
            .ok_or_else(|| error(ErrorCode::WorkflowParallelStepUnsupported))?;
        if descriptors.len() > 16 || remaining_ms > limits.dispatch_timeout_ms {
            return Err(error(ErrorCode::WorkflowParallelStepUnsupported));
        }
        for (index, descriptor) in descriptors.iter().enumerate() {
            descriptor.validate()?;
            if descriptor.config.kind() != WorkflowStepKind::Do
                || descriptor.ordinal != first.ordinal + index as u32
                || descriptor.batch_first_ordinal != first.ordinal
                || descriptor.batch_size != descriptors.len() as u32
                || descriptor.dependencies != first.dependencies
            {
                return Err(error(ErrorCode::WorkflowParallelStepUnsupported));
            }
        }
        let mut conn = self.lock()?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sql_error)?;
        let instance = running(&tx, fence, now_ms)?;
        let metadata = instance
            .durable
            .as_ref()
            .ok_or_else(|| error(ErrorCode::WorkflowCapabilityMismatch))?;
        if metadata.pause_requested || metadata.yield_requested {
            return Ok(descriptors
                .iter()
                .map(|_| WorkflowV2StepGrant::Suspended)
                .collect());
        }
        if first.ordinal == metadata.registered_step_count {
            for descriptor in descriptors {
                if let WorkflowDurableConfig::Do(config) = &descriptor.config
                    && !config.fits_activation(limits.dispatch_timeout_ms)
                {
                    return Err(error(ErrorCode::WorkflowStepConfigUnsupported));
                }
            }
            if descriptors.len() > limits.max_parallel_steps as usize {
                return Err(error(ErrorCode::WorkflowParallelStepUnsupported));
            }
            if first.ordinal + descriptors.len() as u32 > limits.max_steps {
                return Err(error(ErrorCode::WorkflowStepLimitExceeded));
            }
            if metadata.settled_step_count != metadata.registered_step_count {
                return Err(error(ErrorCode::WorkflowParallelStepUnsupported));
            }
            let extra = descriptors.iter().try_fold(0_usize, |bytes, descriptor| {
                Ok::<_, PlatformError>(bytes + descriptor.state_bytes()?)
            })?;
            capacity_v2(
                &tx,
                &instance,
                extra as i64,
                descriptors.len() as i64,
                limits,
            )?;
            for descriptor in descriptors {
                register(&tx, fence, descriptor, now_ms, None, None)?;
            }
        } else if first.ordinal > metadata.registered_step_count {
            return Err(error(ErrorCode::WorkflowNonDeterministic));
        }
        let mut grants = Vec::with_capacity(descriptors.len());
        let mut admitted = 0;
        let mut yielding = false;
        for descriptor in descriptors {
            let step = read_step(
                &tx,
                fence.instance_id,
                fence.instance_generation,
                descriptor.ordinal,
            )?
            .ok_or_else(|| error(ErrorCode::WorkflowInvariantViolation))?;
            if step.descriptor != *descriptor {
                return Err(error(ErrorCode::WorkflowNonDeterministic));
            }
            let grant = match step.state.as_str() {
                "complete" => WorkflowV2StepGrant::Complete,
                "failed" => WorkflowV2StepGrant::Failed,
                "running" => {
                    if step.run_token.as_ref() != Some(&fence.run_token) {
                        return Err(error(ErrorCode::WorkflowStepStale));
                    }
                    if step.deadline.is_none_or(|deadline| deadline <= now_ms) {
                        match durable_settlement::fail(
                            &tx,
                            fence.instance_id,
                            fence.instance_generation,
                            &step,
                            ErrorCode::WorkflowStepTimeout,
                            now_ms,
                        )? {
                            WorkflowV2StepResult::Suspended => WorkflowV2StepGrant::Suspended,
                            _ => WorkflowV2StepGrant::Failed,
                        }
                    } else {
                        admitted += 1;
                        grant(&step, now_ms)?
                    }
                }
                "pending"
                    if step.attempt > 0
                        && step.deadline.is_some_and(|deadline| deadline <= now_ms) =>
                {
                    match durable_settlement::fail(
                        &tx,
                        fence.instance_id,
                        fence.instance_generation,
                        &step,
                        ErrorCode::WorkflowStepTimeout,
                        now_ms,
                    )? {
                        WorkflowV2StepResult::Suspended => WorkflowV2StepGrant::Suspended,
                        _ => WorkflowV2StepGrant::Failed,
                    }
                }
                "pending" | "retry_wait" => {
                    let WorkflowDurableConfig::Do(config) = &step.descriptor.config else {
                        return Err(error(ErrorCode::WorkflowInvariantViolation));
                    };
                    let duration = if step.state == "pending" && step.attempt > 0 {
                        u64::try_from(
                            step.deadline
                                .ok_or_else(|| error(ErrorCode::WorkflowInvariantViolation))?
                                .saturating_sub(now_ms),
                        )
                        .map_err(|_| error(ErrorCode::WorkflowInvariantViolation))?
                    } else {
                        config.timeout
                    };
                    if (step.attempt == 0 || step.state == "retry_wait")
                        && !config.fits_activation(limits.dispatch_timeout_ms)
                    {
                        return Err(error(ErrorCode::WorkflowRuntimeUnavailable));
                    }
                    if step.due.is_some_and(|due| due > now_ms)
                        || admitted >= limits.max_parallel_steps
                        || duration
                            .saturating_add(open_compute_core::workflow::WORKFLOW_DRAIN_MARGIN_MS)
                            > remaining_ms
                    {
                        WorkflowV2StepGrant::Suspended
                    } else {
                        let fresh = step.attempt == 0 || step.state == "retry_wait";
                        let step_token = token()?;
                        if fresh {
                            tx.execute("UPDATE workflow_steps SET state='running',attempt=attempt+1,attempt_started_at_ms=?6,
                                attempt_deadline_at_ms=?7,run_token=?4,step_token=?5,due_at_ms=NULL,error_json=NULL,error_code=NULL,updated_at_ms=?6
                                WHERE instance_id=?1 AND instance_generation=?2 AND ordinal=?3",
                                params![fence.instance_id.to_string(),fence.instance_generation,descriptor.ordinal,fence.run_token.as_bytes().as_slice(),step_token.as_bytes().as_slice(),
                                    now_ms,durable_deadline(now_ms,config.timeout)?]).map_err(sql_error)?;
                        } else {
                            tx.execute("UPDATE workflow_steps SET state='running',run_token=?4,step_token=?5,updated_at_ms=?6
                                WHERE instance_id=?1 AND instance_generation=?2 AND ordinal=?3 AND state='pending'",
                                params![fence.instance_id.to_string(),fence.instance_generation,descriptor.ordinal,fence.run_token.as_bytes().as_slice(),step_token.as_bytes().as_slice(),now_ms]).map_err(sql_error)?;
                        }
                        admitted += 1;
                        WorkflowV2StepGrant::Run {
                            step_token,
                            attempt: step.attempt + u32::from(fresh),
                            remaining_ms: duration,
                            config: config.clone(),
                        }
                    }
                }
                _ => return Err(error(ErrorCode::WorkflowStepStale)),
            };
            yielding |= matches!(grant, WorkflowV2StepGrant::Suspended);
            grants.push(grant);
        }
        if yielding {
            request_yield(&tx, fence, now_ms)?;
        }
        heartbeat(&tx, fence, now_ms, limits)?;
        tx.commit().map_err(sql_error)?;
        Ok(grants)
    }

    /// Fetch one immutable result separately from the batch grant, under the current run lease.
    pub fn workflow_step_result_v2(
        &self,
        fence: &WorkflowFence,
        ordinal: u32,
        now_ms: i64,
    ) -> Result<WorkflowV2StepResult, PlatformError> {
        let conn = self.lock()?;
        let instance = running(&conn, fence, now_ms)?;
        if instance.durable.is_none() {
            return Err(error(ErrorCode::WorkflowCapabilityMismatch));
        }
        result(
            &read_step(&conn, fence.instance_id, fence.instance_generation, ordinal)?
                .ok_or_else(|| error(ErrorCode::WorkflowStepStale))?,
        )
    }
}

pub(super) fn result(step: &DurableStep) -> Result<WorkflowV2StepResult, PlatformError> {
    match step.state.as_str() {
        "complete" => {
            match &step.descriptor.config {
                WorkflowDurableConfig::Do(_) => {
                    let output = step
                        .output
                        .as_deref()
                        .ok_or_else(|| error(ErrorCode::WorkflowInvariantViolation))?;
                    if open_compute_core::workflow::canonical_json(
                        output,
                        ErrorCode::WorkflowInvariantViolation,
                    )
                    .map_err(|_| error(ErrorCode::WorkflowInvariantViolation))?
                        != output
                    {
                        return Err(error(ErrorCode::WorkflowInvariantViolation));
                    }
                }
                WorkflowDurableConfig::WaitEvent { .. } => {
                    open_compute_core::workflow::WorkflowEventEnvelope::from_canonical(
                        step.output
                            .as_deref()
                            .ok_or_else(|| error(ErrorCode::WorkflowInvariantViolation))?,
                    )?;
                }
                _ if step.output.is_some() => {
                    return Err(error(ErrorCode::WorkflowInvariantViolation));
                }
                _ => {}
            }
            Ok(WorkflowV2StepResult::Complete {
                output_json: step.output.clone(),
            })
        }
        "failed" => Ok(WorkflowV2StepResult::Failed {
            code: step
                .failure
                .clone()
                .ok_or_else(|| error(ErrorCode::WorkflowInvariantViolation))?,
        }),
        "waiting" | "retry_wait" | "pending" => Ok(WorkflowV2StepResult::Suspended),
        _ => Err(error(ErrorCode::WorkflowStepStale)),
    }
}

fn grant(step: &DurableStep, now_ms: i64) -> Result<WorkflowV2StepGrant, PlatformError> {
    let WorkflowDurableConfig::Do(config) = &step.descriptor.config else {
        return Err(error(ErrorCode::WorkflowInvariantViolation));
    };
    Ok(WorkflowV2StepGrant::Run {
        step_token: step
            .step_token
            .clone()
            .ok_or_else(|| error(ErrorCode::WorkflowInvariantViolation))?,
        attempt: step.attempt,
        remaining_ms: u64::try_from(
            step.deadline
                .ok_or_else(|| error(ErrorCode::WorkflowInvariantViolation))?
                .saturating_sub(now_ms),
        )
        .map_err(|_| error(ErrorCode::WorkflowInvariantViolation))?,
        config: config.clone(),
    })
}

pub(super) fn register(
    conn: &Connection,
    fence: &WorkflowFence,
    descriptor: &WorkflowStepDescriptor,
    now_ms: i64,
    due: Option<i64>,
    ceiling: Option<i64>,
) -> Result<(), PlatformError> {
    let config = descriptor.config.canonical_json()?;
    let count: u32 = conn
        .query_row(
            "SELECT COUNT(*) FROM workflow_steps WHERE instance_id=?1 AND kind=?2 AND name=?3",
            params![
                fence.instance_id.to_string(),
                descriptor.config.kind().as_str(),
                descriptor.name
            ],
            |row| row.get(0),
        )
        .map_err(sql_error)?;
    let expected: Vec<u32> = if descriptor.batch_first_ordinal == 0 {
        vec![]
    } else {
        let first:u32=conn.query_row("SELECT batch_first_ordinal FROM workflow_steps WHERE instance_id=?1 AND ordinal=?2",
            params![fence.instance_id.to_string(),descriptor.batch_first_ordinal-1],|row|row.get(0)).optional().map_err(sql_error)?
            .ok_or_else(||error(ErrorCode::WorkflowNonDeterministic))?;
        (first..descriptor.batch_first_ordinal).collect()
    };
    if count + 1 != descriptor.name_count || expected != descriptor.dependencies {
        return Err(error(ErrorCode::WorkflowNonDeterministic));
    }
    conn.execute("INSERT INTO workflow_steps(instance_id,instance_generation,ordinal,name,name_count,kind,config_json,descriptor_sha256,
        state,attempt,started_at_ms,updated_at_ms,config_sha256,batch_first_ordinal,batch_size,dependency_count,due_at_ms,event_buffer_ceiling)
        VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,0,?10,?10,?11,?12,?13,?14,?15,?16)",
        params![fence.instance_id.to_string(),fence.instance_generation,descriptor.ordinal,descriptor.name,descriptor.name_count,descriptor.config.kind().as_str(),config.as_bytes(),
            descriptor.sha256()?.as_slice(),if descriptor.config.kind()==WorkflowStepKind::Do {"pending"} else {"waiting"},now_ms,Sha256::digest(config.as_bytes()).as_slice(),
            descriptor.batch_first_ordinal,descriptor.batch_size,descriptor.dependencies.len(),due,ceiling]).map_err(sql_error)?;
    for parent in &descriptor.dependencies {
        conn.execute(
            "INSERT INTO workflow_step_dependencies VALUES(?1,?2,?3,?4)",
            params![
                fence.instance_id.to_string(),
                fence.instance_generation,
                descriptor.ordinal,
                parent
            ],
        )
        .map_err(sql_error)?;
    }
    Ok(())
}

pub(super) fn request_yield(
    conn: &Connection,
    fence: &WorkflowFence,
    now_ms: i64,
) -> Result<(), PlatformError> {
    let changed=conn.execute("UPDATE workflow_instances SET yield_requested=1,updated_at_ms=max(updated_at_ms,?4)
        WHERE id=?1 AND instance_generation=?2 AND run_token=?3 AND state='running' AND run_lease_until_ms>?4",
        params![fence.instance_id.to_string(),fence.instance_generation,fence.run_token.as_bytes().as_slice(),now_ms]).map_err(sql_error)?;
    if changed != 1 {
        return Err(error(ErrorCode::WorkflowRunStale));
    }
    Ok(())
}
