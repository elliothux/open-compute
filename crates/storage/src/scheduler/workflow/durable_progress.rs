//! Bounded monotonic operation decisions; absence alone never authorizes control cleanup.

use super::*;
use crate::{
    WorkflowAppliedOperation, WorkflowOperation, WorkflowOperationKind, WorkflowOperationResult,
    WorkflowRejectedOperation,
};
use open_compute_core::WorkflowOperationId;
use open_compute_core::workflow::WorkflowRestartStepType;

pub(super) fn read_decision(
    conn: &Connection,
    operation: &WorkflowOperation,
) -> Result<Option<WorkflowOperationResult>, PlatformError> {
    let row=conn.query_row("SELECT operation_id,operation_sequence,creation_nonce,expected_generation,target_generation,kind,
        restart_from_name,restart_from_count,restart_from_kind,restart_target_ordinal,outcome,error_code
        FROM workflow_operation_progress WHERE instance_id=?1",[operation.identity.instance_id.to_string()],|row|Ok((
        parse::<WorkflowOperationId>(row,0)?,row.get::<_,i64>(1)?,WorkflowToken::from_bytes(digest(row,2)?),row.get::<_,i64>(3)?,row.get::<_,i64>(4)?,
        row.get::<_,String>(5)?,row.get::<_,Option<String>>(6)?,row.get::<_,Option<u32>>(7)?,row.get::<_,Option<String>>(8)?,
        row.get::<_,Option<u32>>(9)?,row.get::<_,String>(10)?,row.get::<_,Option<String>>(11)?))).optional().map_err(sql_error)?;
    let Some((
        id,
        sequence,
        nonce,
        expected,
        target,
        kind,
        restart_name,
        restart_count,
        restart_kind,
        restart_target,
        outcome,
        code,
    )) = row
    else {
        return Ok(None);
    };
    if sequence < operation.sequence {
        return Ok(None);
    }
    if sequence > operation.sequence {
        return Err(error(ErrorCode::WorkflowRunStale));
    }
    if id != operation.id
        || nonce != operation.identity.creation_nonce
        || expected != operation.identity.instance_generation
        || target != operation.target_generation
        || kind != operation.kind.as_str()
        || restart_name.as_deref()
            != operation
                .restart_from
                .as_ref()
                .map(|selector| selector.name.as_str())
        || restart_count
            != operation
                .restart_from
                .as_ref()
                .map(|selector| selector.count)
        || restart_kind.as_deref()
            != operation
                .restart_from
                .as_ref()
                .and_then(|selector| selector.step_type)
                .map(WorkflowRestartStepType::as_str)
    {
        return Err(error(ErrorCode::WorkflowInvariantViolation));
    }
    verify_one(conn, operation.identity.instance_id)?;
    if outcome == "applied" && code.is_none() {
        if operation.kind == WorkflowOperationKind::Restart {
            let actual = conn
                .query_row(
                    &format!("{INSTANCE_SELECT} WHERE id=?1"),
                    [operation.identity.instance_id.to_string()],
                    instance_row,
                )
                .map_err(sql_error)?;
            let mut expected = operation.identity.clone();
            expected.instance_generation = operation.target_generation;
            if actual.identity != expected {
                return Err(error(ErrorCode::WorkflowInvariantViolation));
            }
            let marker = conn.query_row("SELECT last_restart_operation_id,last_restart_from_name,last_restart_from_count,
                last_restart_from_kind,last_restart_target_ordinal FROM workflow_instances WHERE id=?1",
                [operation.identity.instance_id.to_string()],|row|Ok((row.get::<_,Option<String>>(0)?,row.get::<_,Option<String>>(1)?,
                    row.get::<_,Option<u32>>(2)?,row.get::<_,Option<String>>(3)?,row.get::<_,Option<u32>>(4)?))).map_err(sql_error)?;
            if marker.0.as_deref() != Some(operation.id.to_string().as_str())
                || marker.1.as_deref() != restart_name.as_deref()
                || marker.2 != restart_count
                || marker.3.as_deref() != restart_kind.as_deref()
                || marker.4 != restart_target
            {
                return Err(error(ErrorCode::WorkflowInvariantViolation));
            }
        }
        return Ok(Some(WorkflowOperationResult::Applied(
            WorkflowAppliedOperation {
                operation: operation.clone(),
            },
        )));
    }
    let code = match code.as_deref() {
        Some("WORKFLOW_INSTANCE_NOT_FOUND") => ErrorCode::WorkflowInstanceNotFound,
        Some("WORKFLOW_INSTANCE_STATE_CONFLICT") => ErrorCode::WorkflowInstanceStateConflict,
        Some("WORKFLOW_STATE_QUOTA_EXCEEDED") => ErrorCode::WorkflowStateQuotaExceeded,
        _ => return Err(error(ErrorCode::WorkflowInvariantViolation)),
    };
    if outcome != "rejected" {
        return Err(error(ErrorCode::WorkflowInvariantViolation));
    }
    let instance = conn
        .query_row(
            &format!("{INSTANCE_SELECT} WHERE id=?1"),
            [operation.identity.instance_id.to_string()],
            instance_row,
        )
        .map_err(sql_error)?;
    if instance.identity != operation.identity {
        return Err(error(ErrorCode::WorkflowInvariantViolation));
    }
    Ok(Some(WorkflowOperationResult::Rejected(
        WorkflowRejectedOperation {
            operation: operation.clone(),
            code,
        },
    )))
}

pub(super) fn decide(
    conn: &Connection,
    operation: &WorkflowOperation,
    restart_target: Option<u32>,
    rejection: Option<ErrorCode>,
    now_ms: i64,
) -> Result<WorkflowOperationResult, PlatformError> {
    conn.execute("INSERT INTO workflow_operation_progress(instance_id,operation_id,operation_sequence,creation_nonce,
        expected_generation,target_generation,kind,restart_from_name,restart_from_count,restart_from_kind,restart_target_ordinal,
        outcome,error_code,decided_at_ms) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14)
        ON CONFLICT(instance_id) DO UPDATE SET operation_id=excluded.operation_id,operation_sequence=excluded.operation_sequence,
          creation_nonce=excluded.creation_nonce,expected_generation=excluded.expected_generation,target_generation=excluded.target_generation,
          kind=excluded.kind,restart_from_name=excluded.restart_from_name,restart_from_count=excluded.restart_from_count,
          restart_from_kind=excluded.restart_from_kind,restart_target_ordinal=excluded.restart_target_ordinal,
          outcome=excluded.outcome,error_code=excluded.error_code,decided_at_ms=excluded.decided_at_ms",
        params![operation.identity.instance_id.to_string(),operation.id.to_string(),operation.sequence,
            operation.identity.creation_nonce.as_bytes().as_slice(),operation.identity.instance_generation,operation.target_generation,
            operation.kind.as_str(),operation.restart_from.as_ref().map(|selector|selector.name.as_str()),
            operation.restart_from.as_ref().map(|selector|selector.count),operation.restart_from.as_ref()
                .and_then(|selector|selector.step_type.map(WorkflowRestartStepType::as_str)),restart_target,
            if rejection.is_some(){"rejected"}else{"applied"},rejection.map(ErrorCode::as_str),now_ms]).map_err(sql_error)?;
    read_decision(conn, operation)?.ok_or_else(|| error(ErrorCode::WorkflowInvariantViolation))
}

const INVALID_PROGRESS:&str="SELECT COUNT(*) FROM workflow_operation_progress p WHERE (?1 IS NULL OR p.instance_id=?1) AND NOT (
    (p.outcome='rejected' AND p.error_code IS NOT NULL AND p.error_code IN ('WORKFLOW_INSTANCE_NOT_FOUND','WORKFLOW_INSTANCE_STATE_CONFLICT','WORKFLOW_STATE_QUOTA_EXCEEDED')
      AND EXISTS(SELECT 1 FROM workflow_instances i WHERE i.id=p.instance_id AND i.capability_version=1
      AND i.creation_nonce=p.creation_nonce AND i.instance_generation=p.expected_generation)) OR
    (p.outcome='applied' AND p.error_code IS NULL AND p.kind='restart' AND EXISTS(SELECT 1 FROM workflow_instances i WHERE i.id=p.instance_id
      AND i.capability_version=1 AND i.creation_nonce=p.creation_nonce AND i.instance_generation=p.target_generation
      AND i.last_restart_operation_id=p.operation_id AND i.last_restart_from_name IS p.restart_from_name
      AND i.last_restart_from_count IS p.restart_from_count AND i.last_restart_from_kind IS p.restart_from_kind
      AND i.last_restart_target_ordinal IS p.restart_target_ordinal)) OR
    (p.outcome='applied' AND p.error_code IS NULL AND p.kind='purge' AND NOT EXISTS(SELECT 1 FROM workflow_instances WHERE id=p.instance_id)
      AND EXISTS(SELECT 1 FROM workflow_gc_receipts r WHERE r.instance_id=p.instance_id AND r.operation_id=p.operation_id
        AND r.creation_nonce=p.creation_nonce AND r.instance_generation=p.expected_generation)))";

fn verify_one(conn: &Connection, id: WorkflowInstanceId) -> Result<(), PlatformError> {
    let invalid: u64 = conn
        .query_row(INVALID_PROGRESS, [id.to_string()], |row| row.get(0))
        .map_err(sql_error)?;
    if invalid != 0 {
        return Err(error(ErrorCode::WorkflowInvariantViolation));
    }
    Ok(())
}

pub(super) fn sample_operation_progress(
    conn: &Connection,
    limit: u32,
) -> Result<bool, PlatformError> {
    let mut statement = conn
        .prepare(
            "SELECT instance_id FROM workflow_operation_progress ORDER BY instance_id LIMIT ?1",
        )
        .map_err(sql_error)?;
    let ids = statement
        .query_map([limit], |row| parse::<WorkflowInstanceId>(row, 0))
        .map_err(sql_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(sql_error)?;
    for id in &ids {
        verify_one(conn, *id)?;
    }
    let context: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM workflow_mutation_context)",
            [],
            |row| row.get(0),
        )
        .map_err(sql_error)?;
    if context {
        return Err(error(ErrorCode::WorkflowInvariantViolation));
    }
    Ok(ids.len() == limit as usize)
}

pub(in crate::scheduler) fn verify_operation_progress(
    conn: &Connection,
) -> Result<(), PlatformError> {
    let invalid: u64 = conn
        .query_row(INVALID_PROGRESS, [Option::<String>::None], |row| row.get(0))
        .map_err(sql_error)?;
    let incomplete:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM workflow_mutation_context) OR EXISTS(
        SELECT 1 FROM workflow_gc_receipts r WHERE NOT EXISTS(SELECT 1 FROM workflow_operation_progress p
          WHERE p.instance_id=r.instance_id AND p.operation_id=r.operation_id AND p.outcome='applied' AND p.kind='purge'
            AND p.creation_nonce=r.creation_nonce AND p.expected_generation=r.instance_generation))",[],|row|row.get(0)).map_err(sql_error)?;
    if invalid != 0 || incomplete {
        return Err(error(ErrorCode::WorkflowInvariantViolation));
    }
    Ok(())
}
