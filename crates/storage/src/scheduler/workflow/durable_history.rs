//! Read-only verification of the replay graph and its persisted projections.

use super::*;
use open_compute_core::workflow::{
    WORKFLOW_EVENT_BYTES, WORKFLOW_MAX_SAFE_INTEGER, WorkflowDurableConfig, WorkflowEventEnvelope,
    WorkflowStepDescriptor, WorkflowStepKind,
};
use std::collections::{BTreeMap, BTreeSet};

pub(super) fn verify(
    conn: &Connection,
    instance: &WorkflowInstanceRecord,
) -> Result<(), PlatformError> {
    let metadata = &instance.durable;
    let mut dependencies = dependencies(conn, instance)?;
    let mut statement = conn
        .prepare("SELECT * FROM workflow_steps WHERE instance_id=?1 ORDER BY ordinal LIMIT 1025")
        .map_err(sql_error)?;
    let mut rows = statement
        .query([instance.identity.instance_id.to_string()])
        .map_err(sql_error)?;
    let mut counts = BTreeMap::new();
    let mut consumed = BTreeSet::new();
    let mut registered = 0;
    let mut completed = 0;
    let mut settled = 0;
    let mut next_wake = None;
    let mut batch_end = 0;
    let mut batch_first = 0;
    let mut batch_dependencies = Vec::new();
    let mut settled_ordinals = BTreeSet::new();
    let mut has_pending = false;
    let mut rollback_started = false;
    let mut bytes = initial_state_bytes(&instance.identity, instance.input_json.len()) as u64
        + instance
            .output_json
            .as_ref()
            .map_or(0, |value| value.len() as u64)
        + u64::from(instance.error.is_some()) * failure_json().len() as u64;
    while let Some(row) = rows.next().map_err(sql_error)? {
        let descriptor =
            durable_model::descriptor(row, dependencies.remove(&registered).unwrap_or_default())?;
        let state: String = row.get("state").map_err(sql_error)?;
        let key = (descriptor.config.kind().as_str(), descriptor.name.clone());
        let count = counts.entry(key).or_insert(0);
        *count += 1;
        if descriptor.ordinal != registered || descriptor.name_count != *count {
            return Err(error(ErrorCode::WorkflowInvariantViolation));
        }
        if descriptor.rollback_step {
            rollback_started = true;
            if descriptor.batch_size != 1 || !descriptor.dependencies.is_empty() {
                return Err(error(ErrorCode::WorkflowInvariantViolation));
            }
        } else if rollback_started {
            return Err(error(ErrorCode::WorkflowInvariantViolation));
        }
        if registered == batch_end {
            batch_first = registered;
            batch_end = registered + descriptor.batch_size;
            batch_dependencies = descriptor.dependencies.clone();
        }
        if descriptor.batch_first_ordinal != batch_first
            || descriptor.batch_size != batch_end - batch_first
            || descriptor.dependencies != batch_dependencies
            || descriptor
                .dependencies
                .iter()
                .any(|parent| !settled_ordinals.contains(parent))
        {
            return Err(error(ErrorCode::WorkflowInvariantViolation));
        }
        if row
            .get::<_, Option<i64>>("event_buffer_ceiling")
            .map_err(sql_error)?
            .is_some_and(|ceiling| ceiling >= metadata.next_event_seq)
        {
            return Err(error(ErrorCode::WorkflowInvariantViolation));
        }
        if let Some(wake) = verify_step(row, &descriptor, &state, instance, &mut consumed)? {
            next_wake = Some(next_wake.map_or(wake, |current: i64| current.min(wake)));
        }
        registered += 1;
        completed += u32::from(state == "complete");
        settled += u32::from(matches!(state.as_str(), "complete" | "failed"));
        if matches!(state.as_str(), "complete" | "failed") {
            settled_ordinals.insert(descriptor.ordinal);
        }
        has_pending |= matches!(state.as_str(), "pending" | "running" | "delay_pending");
        bytes += descriptor.state_bytes()? as u64
            + row
                .get::<_, Option<Vec<u8>>>("output_json")
                .map_err(sql_error)?
                .map_or(0, |value| value.len() as u64)
            + row
                .get::<_, Option<Vec<u8>>>("error_json")
                .map_err(sql_error)?
                .map_or(0, |value| value.len() as u64);
    }
    let (event_count, event_bytes) = verify_events(conn, instance, &consumed)?;
    if registered != batch_end
        || !dependencies.is_empty()
        || registered != metadata.registered_step_count
        || completed != instance.completed_step_count
        || settled != metadata.settled_step_count
        || next_wake != metadata.next_wake_at_ms
        || event_count != metadata.event_count
        || event_bytes != metadata.event_bytes
        || bytes + event_bytes != instance.state_bytes
        || (instance.state == WorkflowState::Complete && settled != registered)
        || (instance.state == WorkflowState::Waiting && (next_wake.is_none() || has_pending))
        || ((registered > 0 || instance.state == WorkflowState::Running) && !metadata.has_activated)
        || ((instance.identity.instance_generation == 1)
            != metadata.last_restart_operation_id.is_none())
        || (rollback_started
            && !metadata.rollback_requested
            && instance.state != WorkflowState::Terminated)
    {
        return Err(error(ErrorCode::WorkflowInvariantViolation));
    }
    verify_instance(instance)
}

fn verify_instance(instance: &WorkflowInstanceRecord) -> Result<(), PlatformError> {
    let metadata = &instance.durable;
    for timestamp in [
        Some(instance.identity.created_at_ms),
        Some(instance.updated_at_ms),
        instance.next_run_at_ms,
        instance.run_lease_until_ms,
        instance.terminal_at_ms,
        metadata.next_wake_at_ms,
        metadata.expires_at_ms,
    ]
    .into_iter()
    .flatten()
    {
        safe_time(timestamp)?;
    }
    let expiry = instance
        .terminal_at_ms
        .map(|time| {
            metadata
                .retention
                .expires_at(time, instance.state == WorkflowState::Complete)
        })
        .transpose()
        .map_err(|_| error(ErrorCode::WorkflowInvariantViolation))?;
    if instance.identity.instance_generation < 1
        || expiry != metadata.expires_at_ms
        || instance.state.is_terminal() != instance.terminal_at_ms.is_some()
        || (instance.state == WorkflowState::Complete) != instance.output_json.is_some()
        || (instance.state == WorkflowState::Errored) != instance.error.is_some()
        || instance.error.is_some() != instance.error_code.is_some()
        || (instance.state == WorkflowState::Running) != instance.run_token.is_some()
        || instance.run_token.is_some() != instance.run_lease_until_ms.is_some()
        || (instance.state == WorkflowState::Queued) != instance.next_run_at_ms.is_some()
        || (instance.state != WorkflowState::Running
            && (metadata.pause_requested || metadata.yield_requested))
        || (instance.state.is_terminal() && metadata.rollback_requested)
        || (instance.state.is_terminal() && metadata.next_wake_at_ms.is_some())
        || metadata.next_event_seq < 1
    {
        return Err(error(ErrorCode::WorkflowInvariantViolation));
    }
    Ok(())
}

fn dependencies(
    conn: &Connection,
    instance: &WorkflowInstanceRecord,
) -> Result<BTreeMap<u32, Vec<u32>>, PlatformError> {
    let mut result: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
    let mut statement = conn.prepare("SELECT instance_generation,child_ordinal,parent_ordinal
        FROM workflow_step_dependencies WHERE instance_id=?1 ORDER BY child_ordinal,parent_ordinal LIMIT 16385").map_err(sql_error)?;
    let mut rows = statement
        .query([instance.identity.instance_id.to_string()])
        .map_err(sql_error)?;
    let mut count = 0;
    while let Some(row) = rows.next().map_err(sql_error)? {
        count += 1;
        let generation: i64 = row.get(0).map_err(sql_error)?;
        let child: u32 = row.get(1).map_err(sql_error)?;
        let parent: u32 = row.get(2).map_err(sql_error)?;
        if count > 16384
            || generation != instance.identity.instance_generation
            || child >= 1024
            || parent >= child
        {
            return Err(error(ErrorCode::WorkflowInvariantViolation));
        }
        result.entry(child).or_default().push(parent);
    }
    Ok(result)
}

fn verify_step(
    row: &rusqlite::Row<'_>,
    descriptor: &WorkflowStepDescriptor,
    state: &str,
    instance: &WorkflowInstanceRecord,
    consumed: &mut BTreeSet<i64>,
) -> Result<Option<i64>, PlatformError> {
    let generation: i64 = row.get("instance_generation").map_err(sql_error)?;
    let run_token: Option<Vec<u8>> = row.get("run_token").map_err(sql_error)?;
    let step_token: Option<Vec<u8>> = row.get("step_token").map_err(sql_error)?;
    let due: Option<i64> = row.get("due_at_ms").map_err(sql_error)?;
    let completed: Option<i64> = row.get("completed_at_ms").map_err(sql_error)?;
    let cancelled: Option<i64> = row.get("cancelled_at_ms").map_err(sql_error)?;
    let output: Option<Vec<u8>> = row.get("output_json").map_err(sql_error)?;
    let failure: Option<Vec<u8>> = row.get("error_json").map_err(sql_error)?;
    let code = failure_code(row, "error_code").map_err(sql_error)?;
    let updated: i64 = row.get("updated_at_ms").map_err(sql_error)?;
    for field in [
        "started_at_ms",
        "updated_at_ms",
        "completed_at_ms",
        "cancelled_at_ms",
        "due_at_ms",
        "attempt_started_at_ms",
        "attempt_deadline_at_ms",
    ] {
        if let Some(timestamp) = row.get::<_, Option<i64>>(field).map_err(sql_error)? {
            safe_time(timestamp)?;
        }
    }
    if !matches!(
        state,
        "pending"
            | "running"
            | "delay_pending"
            | "retry_wait"
            | "waiting"
            | "complete"
            | "failed"
            | "cancelled"
    ) || generation != instance.identity.instance_generation
        || (state == "running") != run_token.is_some()
        || (state == "running") != step_token.is_some()
        || (state == "running"
            && (instance.state != WorkflowState::Running
                || run_token.as_deref()
                    != instance
                        .run_token
                        .as_ref()
                        .map(|token| token.as_bytes().as_slice())
                || step_token.as_ref().is_none_or(|token| token.len() != 32)))
        || matches!(state, "complete" | "failed") != completed.is_some()
        || (state == "cancelled") != cancelled.is_some()
        || matches!(state, "waiting" | "retry_wait") != due.is_some()
        || (state == "complete"
            && matches!(
                descriptor.config.kind(),
                WorkflowStepKind::Do | WorkflowStepKind::WaitEvent
            ))
            != output.is_some()
        || matches!(state, "failed" | "delay_pending" | "retry_wait") != failure.is_some()
        || failure.is_some() != code.is_some()
        || failure
            .as_ref()
            .is_some_and(|bytes| bytes != failure_json().as_bytes())
        || completed.is_some_and(|time| time != updated)
        || cancelled.is_some_and(|time| time != updated)
        || (instance.state.is_terminal() && !matches!(state, "complete" | "failed" | "cancelled"))
        || (!instance.state.is_terminal() && state == "cancelled")
    {
        return Err(error(ErrorCode::WorkflowInvariantViolation));
    }
    let attempt: u32 = row.get("attempt").map_err(sql_error)?;
    let started: Option<i64> = row.get("attempt_started_at_ms").map_err(sql_error)?;
    let deadline: Option<i64> = row.get("attempt_deadline_at_ms").map_err(sql_error)?;
    let retry_delay: Option<u64> = row.get("retry_delay_ms").map_err(sql_error)?;
    let ceiling: Option<i64> = row.get("event_buffer_ceiling").map_err(sql_error)?;
    let event_seq: Option<i64> = row.get("consumed_event_seq").map_err(sql_error)?;
    if let WorkflowDurableConfig::Do(config) = &descriptor.config {
        if ceiling.is_some()
            || event_seq.is_some()
            || state == "waiting"
            || (state == "delay_pending"
                && (config.retries.delay.is_some()
                    || due.is_some()
                    || retry_delay.is_some()
                    || code.as_deref() != Some("WORKFLOW_STEP_TIMEOUT")))
            || (state != "retry_wait" && retry_delay.is_some())
            || attempt > config.retries.limit + 1
            || (attempt == 0
                && (!matches!(state, "pending" | "cancelled")
                    || started.is_some()
                    || deadline.is_some()))
            || (attempt > 0
                && (started.is_none()
                    || deadline.is_none()
                    || started.and_then(|time| time.checked_add(config.timeout as i64))
                        != deadline))
            || (state == "complete" && deadline.is_none_or(|time| updated >= time))
            || (state == "retry_wait"
                && (attempt > config.retries.limit
                    || retry_delay.is_none()
                    || updated.checked_add(retry_delay.unwrap_or_default() as i64) != due
                    || config.retries.delay.is_some()
                        && Some(config.retries.delay_after(attempt)?) != retry_delay
                    || !matches!(
                        code.as_deref(),
                        Some("WORKFLOW_EXECUTION_FAILED" | "WORKFLOW_STEP_TIMEOUT")
                    )))
        {
            return Err(error(ErrorCode::WorkflowInvariantViolation));
        }
        if let Some(output) = output {
            let output = std::str::from_utf8(&output)
                .map_err(|_| error(ErrorCode::WorkflowInvariantViolation))?;
            if open_compute_core::workflow::durable_value_base64(
                output,
                ErrorCode::WorkflowInvariantViolation,
            )
            .map_err(|_| error(ErrorCode::WorkflowInvariantViolation))?
                != output
            {
                return Err(error(ErrorCode::WorkflowInvariantViolation));
            }
        }
        Ok(if state == "delay_pending" {
            Some(updated)
        } else if matches!(state, "pending" | "running") {
            deadline
        } else {
            due
        })
    } else {
        if attempt != 0
            || started.is_some()
            || deadline.is_some()
            || !matches!(state, "waiting" | "complete" | "failed" | "cancelled")
        {
            return Err(error(ErrorCode::WorkflowInvariantViolation));
        }
        verify_wait(row, descriptor, state, output.as_deref(), consumed)?;
        Ok(due)
    }
}

fn verify_wait(
    row: &rusqlite::Row<'_>,
    descriptor: &WorkflowStepDescriptor,
    state: &str,
    output: Option<&[u8]>,
    consumed: &mut BTreeSet<i64>,
) -> Result<(), PlatformError> {
    let started: i64 = row.get("started_at_ms").map_err(sql_error)?;
    let updated: i64 = row.get("updated_at_ms").map_err(sql_error)?;
    let due: Option<i64> = row.get("due_at_ms").map_err(sql_error)?;
    let ceiling: Option<i64> = row.get("event_buffer_ceiling").map_err(sql_error)?;
    let sequence: Option<i64> = row.get("consumed_event_seq").map_err(sql_error)?;
    let expected_due = match &descriptor.config {
        WorkflowDurableConfig::Sleep(duration) => started.checked_add(*duration as i64),
        WorkflowDurableConfig::SleepUntil(timestamp) => Some(*timestamp),
        WorkflowDurableConfig::WaitEvent { timeout_ms, .. } => {
            started.checked_add(*timeout_ms as i64)
        }
        WorkflowDurableConfig::Do(_) => return Err(error(ErrorCode::WorkflowInvariantViolation)),
    }
    .ok_or_else(|| error(ErrorCode::WorkflowInvariantViolation))?;
    safe_time(expected_due)?;
    if state == "waiting" && due != Some(expected_due) {
        return Err(error(ErrorCode::WorkflowInvariantViolation));
    }
    if let WorkflowDurableConfig::WaitEvent { event_type, .. } = &descriptor.config {
        let ceiling = ceiling
            .filter(|value| *value >= 0)
            .ok_or_else(|| error(ErrorCode::WorkflowInvariantViolation))?;
        if (state == "complete") != sequence.is_some() {
            return Err(error(ErrorCode::WorkflowInvariantViolation));
        }
        if let Some(sequence) = sequence {
            let output = output
                .and_then(|bytes| std::str::from_utf8(bytes).ok())
                .ok_or_else(|| error(ErrorCode::WorkflowInvariantViolation))?;
            let event = WorkflowEventEnvelope::from_wire(output)?;
            if sequence < 1
                || !consumed.insert(sequence)
                || event.event_type != event_type
                || (event.timestamp_ms >= expected_due && sequence > ceiling)
            {
                return Err(error(ErrorCode::WorkflowInvariantViolation));
            }
        }
        if state == "failed"
            && (updated < expected_due
                || row.get::<_, String>("error_code").map_err(sql_error)?
                    != "WORKFLOW_EVENT_TIMEOUT")
        {
            return Err(error(ErrorCode::WorkflowInvariantViolation));
        }
    } else if ceiling.is_some()
        || sequence.is_some()
        || state == "failed"
        || (state == "complete" && updated < expected_due)
    {
        return Err(error(ErrorCode::WorkflowInvariantViolation));
    }
    Ok(())
}

fn verify_events(
    conn: &Connection,
    instance: &WorkflowInstanceRecord,
    consumed: &BTreeSet<i64>,
) -> Result<(u32, u64), PlatformError> {
    let metadata = &instance.durable;
    let mut statement = conn
        .prepare(
            "SELECT instance_generation,event_seq,type,payload_base64,accepted_at_ms,logical_bytes
        FROM workflow_events WHERE instance_id=?1 ORDER BY event_seq",
        )
        .map_err(sql_error)?;
    let mut rows = statement
        .query([instance.identity.instance_id.to_string()])
        .map_err(sql_error)?;
    let mut count = 0_u32;
    let mut bytes = 0_u64;
    let mut previous = 0;
    while let Some(row) = rows.next().map_err(sql_error)? {
        count = count
            .checked_add(1)
            .ok_or_else(|| error(ErrorCode::WorkflowInvariantViolation))?;
        // The projection bounds streaming even when a corrupt inbox contains extra rows.
        if count > metadata.event_count {
            return Err(error(ErrorCode::WorkflowInvariantViolation));
        }
        let generation: i64 = row.get(0).map_err(sql_error)?;
        let sequence: i64 = row.get(1).map_err(sql_error)?;
        let event_type: String = row.get(2).map_err(sql_error)?;
        let payload = text(row, 3).map_err(sql_error)?;
        let accepted: i64 = row.get(4).map_err(sql_error)?;
        let logical_bytes: u64 = row.get(5).map_err(sql_error)?;
        safe_time(accepted)?;
        open_compute_core::workflow::validate_workflow_event_type(&event_type)
            .map_err(|_| error(ErrorCode::WorkflowInvariantViolation))?;
        if generation != instance.identity.instance_generation
            || sequence <= previous
            || sequence >= metadata.next_event_seq
            || consumed.contains(&sequence)
            || logical_bytes != (WORKFLOW_EVENT_BYTES + event_type.len() + payload.len()) as u64
            || open_compute_core::workflow::durable_value_base64(
                &payload,
                ErrorCode::WorkflowInvariantViolation,
            )
            .map_err(|_| error(ErrorCode::WorkflowInvariantViolation))?
                != payload
        {
            return Err(error(ErrorCode::WorkflowInvariantViolation));
        }
        previous = sequence;
        bytes = bytes
            .checked_add(logical_bytes)
            .ok_or_else(|| error(ErrorCode::WorkflowInvariantViolation))?;
    }
    if consumed
        .last()
        .is_some_and(|sequence| *sequence >= metadata.next_event_seq)
    {
        return Err(error(ErrorCode::WorkflowInvariantViolation));
    }
    Ok((count, bytes))
}

fn safe_time(value: i64) -> Result<(), PlatformError> {
    if value.unsigned_abs() > WORKFLOW_MAX_SAFE_INTEGER {
        return Err(error(ErrorCode::WorkflowInvariantViolation));
    }
    Ok(())
}
