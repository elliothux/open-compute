//! Exact scheduler restart/purge commits under a persisted control intent.

use super::*;
use crate::{WorkflowOperation, WorkflowOperationKind, WorkflowOperationResult};
use open_compute_core::workflow::{WorkflowRestartSelector, WorkflowRestartStepType};

#[derive(Clone, Copy, Debug)]
struct RestartProjection {
    target_ordinal: Option<u32>,
    retain_step_count: u32,
    next_event_seq: i64,
    state_bytes: u64,
    reserved: i64,
}

impl SchedulerStore {
    /// Apply or replay an exact prepared operation, recording both success and definitive rejection.
    /// Unknown I/O failures leave the control intent intact; callers must reconcile this same intent.
    pub fn apply_workflow_operation(
        &self,
        operation: &WorkflowOperation,
        now_ms: i64,
        limits: &WorkflowsConfig,
    ) -> Result<WorkflowOperationResult, PlatformError> {
        limits.validate()?;
        durable_deadline(now_ms, 0)?;
        let mut conn = self.lock()?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sql_error)?;
        if let Some(decision) = durable_progress::read_decision(&tx, operation)? {
            return Ok(decision);
        }
        let instance = tx
            .query_row(
                &format!("{INSTANCE_SELECT} WHERE id=?1"),
                [operation.identity.instance_id.to_string()],
                instance_row,
            )
            .optional()
            .map_err(sql_error)?
            .ok_or_else(|| error(ErrorCode::WorkflowInvariantViolation))?;
        if instance.identity != operation.identity || operation.sequence < 1 {
            return Err(error(ErrorCode::WorkflowRunStale));
        }
        let metadata = &instance.durable;
        inspection::verify_history_connection(&tx, operation.identity.instance_id)?;
        let mut restart = None;
        let rejection = match operation.kind {
            WorkflowOperationKind::Restart
                if metadata
                    .expires_at_ms
                    .is_some_and(|expiry| expiry <= now_ms) =>
            {
                Some(ErrorCode::WorkflowInstanceNotFound)
            }
            WorkflowOperationKind::Purge if !instance.state.is_terminal() => {
                Some(ErrorCode::WorkflowInstanceStateConflict)
            }
            WorkflowOperationKind::Restart => match restart_projection(&tx, &instance, operation) {
                Ok(projection) => match restart_capacity(&tx, &instance, &projection, limits) {
                    Ok(()) => {
                        restart = Some(projection);
                        None
                    }
                    Err(error) => Some(error.code()),
                },
                Err(error) => Some(error.code()),
            },
            WorkflowOperationKind::Purge => None,
        };
        if let Some(code) = rejection {
            if !matches!(
                code,
                ErrorCode::WorkflowInstanceNotFound
                    | ErrorCode::WorkflowInstanceStateConflict
                    | ErrorCode::WorkflowStateQuotaExceeded
            ) {
                return Err(error(code));
            }
            let decision = durable_progress::decide(
                &tx,
                operation,
                restart.and_then(|projection| projection.target_ordinal),
                Some(code),
                now_ms,
            )?;
            tx.commit().map_err(sql_error)?;
            return Ok(decision);
        }
        insert_context(&tx, operation, restart.as_ref(), now_ms)?;
        if let Some(projection) = restart.filter(|projection| projection.retain_step_count > 0) {
            tx.execute_batch(
                "DROP TABLE IF EXISTS temp.workflow_restart_step_snapshot;
                 DROP TABLE IF EXISTS temp.workflow_restart_dependency_snapshot;
                 CREATE TEMP TABLE workflow_restart_step_snapshot AS SELECT * FROM workflow_steps WHERE 0;
                 CREATE TEMP TABLE workflow_restart_dependency_snapshot AS
                   SELECT * FROM workflow_step_dependencies WHERE 0;",
            )
            .map_err(sql_error)?;
            tx.execute(
                "INSERT INTO workflow_restart_step_snapshot SELECT * FROM workflow_steps
                 WHERE instance_id=?1 AND instance_generation=?2 AND ordinal<?3 ORDER BY ordinal",
                params![
                    operation.identity.instance_id.to_string(),
                    operation.identity.instance_generation,
                    projection.retain_step_count
                ],
            )
            .map_err(sql_error)?;
            tx.execute(
                "INSERT INTO workflow_restart_dependency_snapshot SELECT * FROM workflow_step_dependencies
                 WHERE instance_id=?1 AND instance_generation=?2 AND child_ordinal<?3
                 ORDER BY child_ordinal,parent_ordinal",
                params![
                    operation.identity.instance_id.to_string(),
                    operation.identity.instance_generation,
                    projection.retain_step_count
                ],
            )
            .map_err(sql_error)?;
        }
        for table in [
            "workflow_step_dependencies",
            "workflow_steps",
            "workflow_events",
        ] {
            tx.execute(
                &format!("DELETE FROM {table} WHERE instance_id=?1 AND instance_generation=?2"),
                params![
                    operation.identity.instance_id.to_string(),
                    operation.identity.instance_generation
                ],
            )
            .map_err(sql_error)?;
        }
        match operation.kind {
            WorkflowOperationKind::Restart => {
                let projection =
                    restart.ok_or_else(|| error(ErrorCode::WorkflowInvariantViolation))?;
                tx.execute("UPDATE workflow_instances SET instance_generation=?2,state='queued',next_run_at_ms=?3,
                    output_json=NULL,error_json=NULL,error_code=NULL,run_token=NULL,run_claimed_at_ms=NULL,run_lease_until_ms=NULL,
                    pause_requested=0,yield_requested=0,rollback_requested=0,terminal_at_ms=NULL,expires_at_ms=NULL,next_event_seq=?4,
                    has_activated=(?5>0),last_restart_operation_id=?6,last_restart_from_name=?7,
                    last_restart_from_count=?8,last_restart_from_kind=?9,last_restart_target_ordinal=?10,
                    state_bytes=?11,updated_at_ms=?3 WHERE id=?1",
                    params![operation.identity.instance_id.to_string(),operation.target_generation,now_ms,
                        projection.next_event_seq,projection.retain_step_count,operation.id.to_string(),
                        operation.restart_from.as_ref().map(|selector|selector.name.as_str()),
                        operation.restart_from.as_ref().map(|selector|selector.count),
                        operation.restart_from.as_ref().and_then(|selector|selector.step_type.map(WorkflowRestartStepType::as_str)),
                        projection.target_ordinal,initial_state_bytes(&instance.identity,instance.input_json.len())]).map_err(sql_error)?;
                if projection.retain_step_count > 0 {
                    let target = projection
                        .target_ordinal
                        .ok_or_else(|| error(ErrorCode::WorkflowInvariantViolation))?;
                    tx.execute("INSERT INTO workflow_steps(instance_id,instance_generation,ordinal,name,name_count,kind,config_json,
                        descriptor_sha256,state,attempt,run_token,step_token,output_json,error_json,error_code,started_at_ms,
                        updated_at_ms,completed_at_ms,config_sha256,batch_first_ordinal,batch_size,dependency_count,
                        attempt_started_at_ms,attempt_deadline_at_ms,due_at_ms,retry_delay_ms,cancelled_at_ms,
                        event_buffer_ceiling,consumed_event_seq)
                      SELECT instance_id,?2,ordinal,name,name_count,kind,config_json,descriptor_sha256,
                        CASE WHEN ordinal<?3 THEN state WHEN kind='do' THEN 'pending' ELSE 'waiting' END,
                        CASE WHEN ordinal<?3 THEN attempt ELSE 0 END,NULL,NULL,
                        CASE WHEN ordinal<?3 THEN output_json ELSE NULL END,NULL,NULL,
                        CASE WHEN ordinal<?3 THEN started_at_ms ELSE ?4 END,
                        CASE WHEN ordinal<?3 THEN updated_at_ms ELSE ?4 END,
                        CASE WHEN ordinal<?3 THEN completed_at_ms ELSE NULL END,
                        config_sha256,batch_first_ordinal,batch_size,dependency_count,
                        CASE WHEN ordinal<?3 THEN attempt_started_at_ms ELSE NULL END,
                        CASE WHEN ordinal<?3 THEN attempt_deadline_at_ms ELSE NULL END,
                        CASE WHEN ordinal<?3 THEN due_at_ms WHEN kind='sleep' THEN ?4+json_extract(CAST(config_json AS TEXT),'$.durationMs')
                          WHEN kind='sleep_until' THEN json_extract(CAST(config_json AS TEXT),'$.timestampMs')
                          WHEN kind='wait_event' THEN ?4+json_extract(CAST(config_json AS TEXT),'$.timeoutMs') ELSE NULL END,
                        CASE WHEN ordinal<?3 THEN retry_delay_ms ELSE NULL END,
                        CASE WHEN ordinal<?3 THEN cancelled_at_ms ELSE NULL END,
                        CASE WHEN ordinal<?3 THEN event_buffer_ceiling WHEN kind='wait_event' THEN ?5-1 ELSE NULL END,
                        CASE WHEN ordinal<?3 THEN consumed_event_seq ELSE NULL END
                      FROM workflow_restart_step_snapshot ORDER BY ordinal",
                        params![operation.identity.instance_id.to_string(),operation.target_generation,target,now_ms,projection.next_event_seq]
                    ).map_err(sql_error)?;
                    tx.execute("INSERT INTO workflow_step_dependencies(instance_id,instance_generation,child_ordinal,parent_ordinal)
                        SELECT instance_id,?2,child_ordinal,parent_ordinal FROM workflow_restart_dependency_snapshot
                        ORDER BY child_ordinal,parent_ordinal",
                        params![operation.identity.instance_id.to_string(),operation.target_generation]
                    ).map_err(sql_error)?;
                    tx.execute_batch("DROP TABLE workflow_restart_dependency_snapshot; DROP TABLE workflow_restart_step_snapshot;")
                        .map_err(sql_error)?;
                }
                inspection::verify_history_connection(&tx, operation.identity.instance_id)?;
            }
            WorkflowOperationKind::Purge => {
                tx.execute(
                    "DELETE FROM workflow_event_receipts WHERE instance_id=?1",
                    [operation.identity.instance_id.to_string()],
                )
                .map_err(sql_error)?;
                tx.execute(
                    "DELETE FROM workflow_instances WHERE id=?1",
                    [operation.identity.instance_id.to_string()],
                )
                .map_err(sql_error)?;
                tx.execute("INSERT INTO workflow_gc_receipts(operation_id,instance_id,creation_nonce,creation_operation_id,
                    instance_generation,deleted_at_ms) VALUES(?1,?2,?3,?4,?5,?6)",params![operation.id.to_string(),
                        operation.identity.instance_id.to_string(),operation.identity.creation_nonce.as_bytes().as_slice(),
                        operation.identity.creation_operation_id.to_string(),operation.identity.instance_generation,now_ms]).map_err(sql_error)?;
            }
        }
        let decision = durable_progress::decide(
            &tx,
            operation,
            restart.and_then(|projection| projection.target_ordinal),
            None,
            now_ms,
        )?;
        tx.execute(
            "DELETE FROM workflow_mutation_context WHERE instance_id=?1",
            [operation.identity.instance_id.to_string()],
        )
        .map_err(sql_error)?;
        tx.commit().map_err(sql_error)?;
        self.wake.notify();
        Ok(decision)
    }

    /// Read an exact committed operation result without applying, repairing or deleting anything.
    pub fn workflow_operation_result(
        &self,
        operation: &WorkflowOperation,
    ) -> Result<Option<WorkflowOperationResult>, PlatformError> {
        let conn = self.lock()?;
        durable_progress::read_decision(&conn, operation)
    }

    /// Whether the current generation is the exact committed result of this restart invocation.
    pub fn workflow_restart_matches(
        &self,
        instance: WorkflowInstanceId,
        operation: WorkflowOperationId,
        restart_from: Option<&WorkflowRestartSelector>,
    ) -> Result<bool, PlatformError> {
        let conn = self.lock()?;
        let marker = conn
            .query_row(
                "SELECT last_restart_operation_id,last_restart_from_name,last_restart_from_count,last_restart_from_kind
                 FROM workflow_instances WHERE id=?1",
                [instance.to_string()],
                |row| {
                    Ok((
                        row.get::<_, Option<String>>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, Option<u32>>(2)?,
                        row.get::<_, Option<String>>(3)?,
                    ))
                },
            )
            .optional()
            .map_err(sql_error)?;
        let Some((id, name, count, kind)) = marker else {
            return Ok(false);
        };
        if id.as_deref() != Some(&operation.to_string()) {
            return Ok(false);
        }
        let expected_name = restart_from.map(|selector| selector.name.as_str());
        let expected_count = restart_from.map(|selector| selector.count);
        let expected_kind = restart_from
            .and_then(|selector| selector.step_type)
            .map(WorkflowRestartStepType::as_str);
        if name.as_deref() != expected_name
            || count != expected_count
            || kind.as_deref() != expected_kind
        {
            return Err(error(ErrorCode::WorkflowInvariantViolation));
        }
        inspection::verify_history_connection(&conn, instance)?;
        Ok(true)
    }
}

fn insert_context(
    conn: &Connection,
    operation: &WorkflowOperation,
    restart: Option<&RestartProjection>,
    now_ms: i64,
) -> Result<(), PlatformError> {
    conn.execute("INSERT INTO workflow_mutation_context(instance_id,operation_id,creation_nonce,expected_generation,target_generation,kind,
        restart_from_name,restart_from_count,restart_from_kind,restart_target_ordinal,restart_retain_step_count,restart_next_event_seq,authorized_at_ms)
        VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)",params![operation.identity.instance_id.to_string(),operation.id.to_string(),
            operation.identity.creation_nonce.as_bytes().as_slice(),operation.identity.instance_generation,operation.target_generation,
            operation.kind.as_str(),operation.restart_from.as_ref().map(|selector|selector.name.as_str()),
            operation.restart_from.as_ref().map(|selector|selector.count),operation.restart_from.as_ref()
                .and_then(|selector|selector.step_type.map(WorkflowRestartStepType::as_str)),
            restart.and_then(|projection|projection.target_ordinal),restart.map(|projection|projection.retain_step_count),
            restart.map(|projection|projection.next_event_seq),now_ms]).map_err(sql_error)?;
    Ok(())
}

fn restart_capacity(
    conn: &Connection,
    instance: &WorkflowInstanceRecord,
    projection: &RestartProjection,
    limits: &WorkflowsConfig,
) -> Result<(), PlatformError> {
    if instance.state.is_terminal() {
        let active:u64=conn.query_row("SELECT COUNT(*) FROM workflow_instances WHERE account_id=?1 AND state IN ('queued','running','waiting','paused')",
            [instance.identity.target.account_id.to_string()],|row|row.get(0)).map_err(sql_error)?;
        if active >= u64::from(limits.max_active_per_account) {
            return Err(error(ErrorCode::WorkflowStateQuotaExceeded));
        }
    }
    let reserved:i64=conn.query_row("SELECT (state IN ('queued','running','waiting','paused')) + (SELECT COUNT(*) FROM workflow_steps
        WHERE instance_id=?1 AND kind IN ('do','wait_event') AND state IN ('pending','running','delay_pending','waiting')) FROM workflow_instances WHERE id=?1",
        [instance.identity.instance_id.to_string()],|row|row.get(0)).map_err(sql_error)?;
    let projected = i64::try_from(projection.state_bytes)
        .map_err(|_| error(ErrorCode::WorkflowInvariantViolation))?;
    let current = i64::try_from(instance.state_bytes)
        .map_err(|_| error(ErrorCode::WorkflowInvariantViolation))?;
    capacity_change(
        conn,
        instance,
        projected - current,
        projection.reserved - reserved,
        limits,
    )
}

fn restart_projection(
    conn: &Connection,
    instance: &WorkflowInstanceRecord,
    operation: &WorkflowOperation,
) -> Result<RestartProjection, PlatformError> {
    let base = initial_state_bytes(&instance.identity, instance.input_json.len()) as u64;
    let Some(selector) = operation.restart_from.as_ref() else {
        return Ok(RestartProjection {
            target_ordinal: None,
            retain_step_count: 0,
            next_event_seq: 1,
            state_bytes: base,
            reserved: 1,
        });
    };
    selector.validate()?;
    let selected_kind = selector.step_type.map(WorkflowRestartStepType::as_str);
    let mut statement = conn
        .prepare(
            "SELECT ordinal,batch_first_ordinal,batch_size FROM workflow_steps
             WHERE instance_id=?1 AND instance_generation=?2 AND name=?3 AND name_count=?4
               AND json_extract(CAST(config_json AS TEXT),'$.rollbackStep')=0
               AND (?5 IS NULL OR (?5='do' AND kind='do') OR (?5='sleep' AND kind IN ('sleep','sleep_until'))
                 OR (?5='waitForEvent' AND kind='wait_event')) ORDER BY ordinal LIMIT 2",
        )
        .map_err(sql_error)?;
    let matches = statement
        .query_map(
            params![
                instance.identity.instance_id.to_string(),
                instance.identity.instance_generation,
                selector.name,
                selector.count,
                selected_kind
            ],
            |row| {
                Ok((
                    row.get::<_, u32>(0)?,
                    row.get::<_, u32>(1)?,
                    row.get::<_, u32>(2)?,
                ))
            },
        )
        .map_err(sql_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(sql_error)?;
    let [(target, batch_first, batch_size)] = matches.as_slice() else {
        return Err(error(ErrorCode::WorkflowInstanceStateConflict));
    };
    let retain_step_count = batch_first
        .checked_add(*batch_size)
        .filter(|count| *count > *target && *count <= 1024)
        .ok_or_else(|| error(ErrorCode::WorkflowInvariantViolation))?;
    let (complete_prefix, retained): (u32, u32) = conn
        .query_row(
            "SELECT coalesce(SUM(ordinal<?3 AND state='complete'),0),coalesce(SUM(ordinal<?4),0)
             FROM workflow_steps WHERE instance_id=?1 AND instance_generation=?2",
            params![
                instance.identity.instance_id.to_string(),
                instance.identity.instance_generation,
                target,
                retain_step_count
            ],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(sql_error)?;
    if complete_prefix != *target || retained != retain_step_count {
        return Err(error(ErrorCode::WorkflowInstanceStateConflict));
    }
    let (history_bytes, unfinished): (u64, i64) = conn
        .query_row(
            "SELECT coalesce(SUM(160+length(CAST(name AS BLOB))+length(config_json)+16*dependency_count
                +CASE WHEN ordinal<?3 THEN coalesce(length(output_json),0) ELSE 0 END),0),
               coalesce(SUM(ordinal>=?3 AND kind IN ('do','wait_event')),0)
             FROM workflow_steps WHERE instance_id=?1 AND instance_generation=?2 AND ordinal<?4",
            params![
                instance.identity.instance_id.to_string(),
                instance.identity.instance_generation,
                target,
                retain_step_count
            ],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(sql_error)?;
    Ok(RestartProjection {
        target_ordinal: Some(*target),
        retain_step_count,
        next_event_seq: instance.durable.next_event_seq,
        state_bytes: base
            .checked_add(history_bytes)
            .ok_or_else(|| error(ErrorCode::WorkflowInvariantViolation))?,
        reserved: 1_i64
            .checked_add(unfinished)
            .ok_or_else(|| error(ErrorCode::WorkflowInvariantViolation))?,
    })
}
