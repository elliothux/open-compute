//! Durable sleep/event registration and one transaction-level FIFO/timeout arbiter.

use super::durable_model::{DurableStep, read_step};
use super::*;
use open_compute_core::workflow::{
    WORKFLOW_EVENT_BYTES, WorkflowDurableConfig, WorkflowEventEnvelope,
};

impl SchedulerStore {
    /// Admit a canonical event for an exact immutable instance generation, then arbitrate a matching wait.
    /// Inbox insertion, result copy and consumption commit together; ambiguous callers must not auto-retry.
    pub fn send_workflow_event(
        &self,
        identity: &WorkflowInstanceIdentity,
        operation_id: WorkflowOperationId,
        event_type: &str,
        payload: &str,
        now_ms: i64,
        limits: &WorkflowsConfig,
    ) -> Result<(), PlatformError> {
        limits.validate()?;
        open_compute_core::workflow::validate_workflow_event_type(event_type)?;
        let payload = open_compute_core::workflow::durable_value_base64(
            payload,
            ErrorCode::WorkflowPayloadTooLarge,
        )?;
        let mut conn = self.lock()?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sql_error)?;
        let payload_sha256: [u8; 32] = Sha256::digest(payload.as_bytes()).into();
        let receipt = tx.query_row(
            "SELECT instance_id,instance_generation,type,payload_sha256 FROM workflow_event_receipts WHERE operation_id=?1",
            [operation_id.to_string()],
            |row| Ok((parse::<WorkflowInstanceId>(row, 0)?, row.get::<_, i64>(1)?, row.get::<_, String>(2)?, row.get::<_, Vec<u8>>(3)?)),
        ).optional().map_err(sql_error)?;
        if let Some((stored_id, generation, stored_type, stored_payload)) = receipt {
            if stored_id != identity.instance_id
                || generation != identity.instance_generation
                || stored_type != event_type
                || stored_payload.as_slice() != payload_sha256
            {
                return Err(error(ErrorCode::WorkflowInvariantViolation));
            }
            return Ok(());
        }
        let instance = tx
            .query_row(
                &format!("{INSTANCE_SELECT} WHERE id=?1"),
                [identity.instance_id.to_string()],
                instance_row,
            )
            .optional()
            .map_err(sql_error)?
            .ok_or_else(|| error(ErrorCode::WorkflowInstanceNotFound))?;
        if instance.identity != *identity {
            return Err(error(ErrorCode::WorkflowInstanceStateConflict));
        }
        let metadata = &instance.durable;
        if instance.state.is_terminal() {
            return Err(error(ErrorCode::WorkflowInstanceStateConflict));
        }
        let logical = WORKFLOW_EVENT_BYTES + event_type.len() + payload.len();
        if metadata.event_count >= limits.max_buffered_events
            || metadata.event_bytes.saturating_add(logical as u64) > limits.max_event_bytes
            || metadata.next_event_seq == i64::MAX
        {
            return Err(error(ErrorCode::WorkflowEventQueueFull));
        }
        capacity_change(&tx, &instance, logical as i64, 0, limits)?;
        let now_ms = now_ms.max(instance.updated_at_ms);
        durable_deadline(now_ms, 0)?;
        tx.execute("INSERT INTO workflow_event_receipts(operation_id,instance_id,instance_generation,type,payload_sha256,accepted_at_ms)
            VALUES(?1,?2,?3,?4,?5,?6)", params![operation_id.to_string(),identity.instance_id.to_string(),identity.instance_generation,
                event_type,payload_sha256.as_slice(),now_ms]).map_err(sql_error)?;
        tx.execute("INSERT INTO workflow_events(instance_id,instance_generation,event_seq,type,payload_base64,accepted_at_ms,logical_bytes)
            VALUES(?1,?2,?3,?4,?5,?6,?7)",params![identity.instance_id.to_string(),identity.instance_generation,metadata.next_event_seq,
                event_type,payload.as_bytes(),now_ms,logical]).map_err(sql_error)?;
        tx.execute(
            "UPDATE workflow_instances SET updated_at_ms=?2 WHERE id=?1",
            params![identity.instance_id.to_string(), now_ms],
        )
        .map_err(sql_error)?;
        let ordinal:Option<u32>=tx.query_row("SELECT ordinal FROM workflow_steps WHERE instance_id=?1 AND state='waiting'
            AND kind='wait_event' AND json_extract(CAST(config_json AS TEXT),'$.type')=?2 ORDER BY ordinal LIMIT 1",
            params![identity.instance_id.to_string(),event_type],|row|row.get(0)).optional().map_err(sql_error)?;
        if let Some(ordinal) = ordinal {
            let step = read_step(
                &tx,
                identity.instance_id,
                identity.instance_generation,
                ordinal,
            )?
            .ok_or_else(|| error(ErrorCode::WorkflowInvariantViolation))?;
            if !matches!(
                settle(&tx, &instance, &step, now_ms, limits)?,
                WorkflowStepResult::Suspended
            ) {
                wake(&tx, identity.instance_id, now_ms)?;
            }
        }
        tx.commit().map_err(sql_error)?;
        self.wake.notify();
        Ok(())
    }
}

pub(super) fn settle(
    conn: &Connection,
    instance: &WorkflowInstanceRecord,
    step: &DurableStep,
    now_ms: i64,
    limits: &WorkflowsConfig,
) -> Result<WorkflowStepResult, PlatformError> {
    if step.state != "waiting" {
        return durable_steps::result(step);
    }
    let due = step
        .due
        .ok_or_else(|| error(ErrorCode::WorkflowInvariantViolation))?;
    let id = instance.identity.instance_id;
    let ordinal = step.descriptor.ordinal;
    if let WorkflowDurableConfig::WaitEvent { event_type, .. } = &step.descriptor.config {
        let event: Option<(i64, String, i64, u64)> = conn
            .query_row(
                "SELECT event_seq,payload_base64,accepted_at_ms,logical_bytes FROM workflow_events
            WHERE instance_id=?1 AND instance_generation=?2 AND type=?3 ORDER BY event_seq LIMIT 1",
                params![
                    id.to_string(),
                    instance.identity.instance_generation,
                    event_type
                ],
                |row| Ok((row.get(0)?, text(row, 1)?, row.get(2)?, row.get(3)?)),
            )
            .optional()
            .map_err(sql_error)?;
        if let Some((sequence, payload, accepted, bytes)) = event {
            let ceiling = step
                .ceiling
                .ok_or_else(|| error(ErrorCode::WorkflowInvariantViolation))?;
            if accepted < due || sequence <= ceiling {
                let output = WorkflowEventEnvelope {
                    event_type,
                    payload_base64: &payload,
                    timestamp_ms: accepted,
                }
                .canonical_wire()?;
                capacity_change(
                    conn,
                    instance,
                    output.len() as i64
                        - i64::try_from(bytes)
                            .map_err(|_| error(ErrorCode::WorkflowInvariantViolation))?,
                    -1,
                    limits,
                )?;
                conn.execute("UPDATE workflow_steps SET state='complete',output_json=?3,consumed_event_seq=?4,due_at_ms=NULL,updated_at_ms=?5,completed_at_ms=?5
                    WHERE instance_id=?1 AND ordinal=?2",params![id.to_string(),ordinal,output.as_bytes(),sequence,now_ms]).map_err(sql_error)?;
                conn.execute(
                    "DELETE FROM workflow_events WHERE instance_id=?1 AND event_seq=?2",
                    params![id.to_string(), sequence],
                )
                .map_err(sql_error)?;
                return Ok(WorkflowStepResult::Event {
                    event_type: event_type.clone(),
                    payload_base64: payload,
                    timestamp_ms: accepted,
                });
            }
        }
        if due <= now_ms {
            conn.execute("UPDATE workflow_steps SET state='failed',error_json=?3,error_code='WORKFLOW_EVENT_TIMEOUT',due_at_ms=NULL,updated_at_ms=?4,completed_at_ms=?4
                WHERE instance_id=?1 AND ordinal=?2",params![id.to_string(),ordinal,failure_json().as_bytes(),now_ms]).map_err(sql_error)?;
            return Ok(WorkflowStepResult::Failed {
                code: ErrorCode::WorkflowEventTimeout.as_str().into(),
            });
        }
    } else if due <= now_ms {
        conn.execute("UPDATE workflow_steps SET state='complete',due_at_ms=NULL,updated_at_ms=?3,completed_at_ms=?3
            WHERE instance_id=?1 AND ordinal=?2",params![id.to_string(),ordinal,now_ms]).map_err(sql_error)?;
        return Ok(WorkflowStepResult::Complete {
            output_base64: None,
        });
    }
    Ok(WorkflowStepResult::Suspended)
}

pub(super) fn wake(
    conn: &Connection,
    id: WorkflowInstanceId,
    now_ms: i64,
) -> Result<(), PlatformError> {
    conn.execute("UPDATE workflow_instances SET state='queued',next_run_at_ms=?2,updated_at_ms=max(updated_at_ms,?2)
        WHERE id=?1 AND state='waiting'",params![id.to_string(),now_ms]).map_err(sql_error)?;
    Ok(())
}
