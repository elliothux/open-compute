use super::*;
use open_compute_core::WorkloadSummary;

impl SchedulerStore {
    /// Page through one account/definition without reading payloads or private fences.
    pub fn inspect_workflow_instances(
        &self,
        account: open_compute_core::AccountId,
        definition: open_compute_core::WorkflowId,
        after: Option<WorkflowInstanceId>,
        limit: u32,
        now_ms: i64,
    ) -> Result<Vec<WorkflowInstanceInspection>, PlatformError> {
        bounded(limit)?;
        let conn = self.lock()?;
        let mut statement = conn
            .prepare(&format!(
                "{INSTANCE_INSPECTION_SELECT}
            WHERE account_id=?1 AND definition_id=?2 AND (?3 IS NULL OR id>?3)
              AND (expires_at_ms IS NULL OR expires_at_ms>?5) ORDER BY id LIMIT ?4"
            ))
            .map_err(sql_error)?;
        statement
            .query_map(
                params![
                    account.to_string(),
                    definition.to_string(),
                    after.map(|id| id.to_string()),
                    limit,
                    now_ms
                ],
                inspection_row,
            )
            .map_err(sql_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(sql_error)
    }

    /// Read one exact instance's metadata without loading input, output or private tokens.
    pub fn inspect_workflow_instance(
        &self,
        id: WorkflowInstanceId,
        now_ms: i64,
    ) -> Result<Option<WorkflowInstanceInspection>, PlatformError> {
        self.lock()?.query_row(
            &format!("{INSTANCE_INSPECTION_SELECT} WHERE id=?1 AND (expires_at_ms IS NULL OR expires_at_ms>?5)"),
            params![id.to_string(), Option::<String>::None, Option::<String>::None, 1, now_ms],
            inspection_row,
        ).optional().map_err(sql_error)
    }

    /// Aggregate low-cardinality counts without reading retained payloads or exceptions.
    pub fn inspect_workflows(&self, now_ms: i64) -> Result<WorkflowInspection, PlatformError> {
        workflow_inspection_connection(&*self.lock()?, now_ms)
    }

    /// Pool-ready work and earliest persisted lease/backoff deadline.
    pub fn workflow_workload_summary(&self, now_ms: i64) -> Result<WorkloadSummary, PlatformError> {
        self.lock()?.query_row("SELECT coalesce(SUM((state='queued' AND next_run_at_ms<=?1) OR (state='waiting' AND next_wake_at_ms<=?1)),0),coalesce(SUM(state='running'),0),
            coalesce(SUM(state='running' AND run_lease_until_ms<=?1),0),MIN(CASE WHEN state='waiting' THEN next_wake_at_ms ELSE next_run_at_ms END),
            MIN(CASE WHEN state='waiting' THEN next_wake_at_ms ELSE coalesce(next_run_at_ms,run_lease_until_ms) END) FROM workflow_instances",[now_ms],|row|Ok(WorkloadSummary {
                ready: row.get(0)?,claimed: row.get(1)?,expired: row.get(2)?,oldest_due_at_ms: row.get(3)?,next_due_at_ms: row.get(4)?,
            })).map_err(sql_error)
    }

    /// Inspect bounded step metadata without loading result bytes, errors, or private fences.
    pub fn workflow_steps(
        &self,
        id: WorkflowInstanceId,
        after: Option<u32>,
        limit: u32,
    ) -> Result<Vec<WorkflowStepInspection>, PlatformError> {
        bounded(limit)?;
        let conn = self.lock()?;
        let mut statement = conn
            .prepare(
                "SELECT s.ordinal,s.name,s.name_count,s.state,coalesce(length(s.output_json),0),s.error_code,i.capability_version,
                s.kind,s.attempt,s.attempt_deadline_at_ms,s.due_at_ms,s.batch_first_ordinal,s.batch_size
            FROM workflow_steps s JOIN workflow_instances i ON i.id=s.instance_id
            WHERE s.instance_id=?1 AND s.ordinal>?2 ORDER BY s.ordinal LIMIT ?3",
            )
            .map_err(sql_error)?;
        statement
            .query_map(
                params![id.to_string(), after.map_or(-1, i64::from), limit],
                |row| {
                    Ok(WorkflowStepInspection {
                        instance_id: id,
                        ordinal: row.get(0)?,
                        name: row.get(1)?,
                        name_count: row.get(2)?,
                        state: row.get(3)?,
                        output_bytes: row.get(4)?,
                        error_code: failure_code(row, 5)?,
                        kind: row.get(7)?,
                        attempt: row.get(8)?,
                        attempt_deadline_at_ms: row.get(9)?,
                        due_at_ms: row.get(10)?,
                        batch_first_ordinal: row.get(11)?,
                        batch_size: row.get(12)?,
                    })
                },
            )
            .map_err(sql_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(sql_error)
    }

    /// Validate one bounded history against descriptor, ordinal, and accounting authority.
    /// Corruption is reported, never repaired by deleting history or replaying callbacks.
    pub fn verify_workflow_history(&self, id: WorkflowInstanceId) -> Result<(), PlatformError> {
        let conn = self.lock()?;
        verify_history_connection(&conn, id)
    }
}

const INSTANCE_INSPECTION_SELECT: &str = "SELECT id,external_instance_id,version_id,deployment_id,class_name,instance_generation,state,
    completed_step_count,(SELECT COUNT(*) FROM workflow_steps s WHERE s.instance_id=i.id),state_bytes,
    CASE WHEN run_lease_until_ms IS NOT NULL THEN MAX(0,run_lease_until_ms-?5) END,created_at_ms,terminal_at_ms,error_code,capability_version,
    pause_requested,yield_requested,next_wake_at_ms,registered_step_count,settled_step_count,success_retention_ms,error_retention_ms,
    expires_at_ms,last_restart_operation_id,event_count,event_bytes,next_event_seq,has_activated,
    rollback_requested FROM workflow_instances i";

fn inspection_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<WorkflowInstanceInspection> {
    let state: String = row.get(6)?;
    let capability: i64 = row.get(14)?;
    if capability != 1 {
        return Err(rusqlite::Error::InvalidQuery);
    }
    let status = match state.as_str() {
        "queued" => WorkflowState::Queued,
        "running" => WorkflowState::Running,
        "complete" => WorkflowState::Complete,
        "errored" => WorkflowState::Errored,
        "waiting" => WorkflowState::Waiting,
        "paused" => WorkflowState::Paused,
        "terminated" => WorkflowState::Terminated,
        _ => return Err(rusqlite::Error::InvalidQuery),
    };
    Ok(WorkflowInstanceInspection {
        id: parse(row, 0)?,
        external_instance_id: row.get(1)?,
        version_id: parse(row, 2)?,
        deployment_id: parse(row, 3)?,
        class_name: row.get(4)?,
        generation: row.get(5)?,
        status,
        completed_step_count: row.get(7)?,
        step_count: row.get(8)?,
        state_bytes: row.get(9)?,
        lease_remaining_ms: row.get(10)?,
        created_at_ms: row.get(11)?,
        terminal_at_ms: row.get(12)?,
        error_code: failure_code(row, 13)?,
        capability_version: row.get(14)?,
        durable: durable_state(row)?,
    })
}

pub(super) fn verify_history_connection(
    conn: &Connection,
    id: WorkflowInstanceId,
) -> Result<(), PlatformError> {
    let instance = conn
        .query_row(
            &format!("{INSTANCE_SELECT} WHERE id=?1"),
            [id.to_string()],
            instance_row,
        )
        .optional()
        .map_err(sql_error)?
        .ok_or_else(|| error(ErrorCode::WorkflowInstanceNotFound))?;
    if crate::workflows::helpers::version_digest(&instance.identity.target)?
        != instance.identity.target.descriptor_sha256
    {
        return Err(error(ErrorCode::WorkflowInvariantViolation));
    }
    for json in std::iter::once(instance.input_json.as_str()).chain(instance.output_json.as_deref())
    {
        if open_compute_core::workflow::durable_value_base64(
            json,
            ErrorCode::WorkflowInvariantViolation,
        )
        .map_err(|_| error(ErrorCode::WorkflowInvariantViolation))?
            != json
        {
            return Err(error(ErrorCode::WorkflowInvariantViolation));
        }
    }
    durable_history::verify(conn, &instance)
}

pub(crate) fn workflow_inspection_connection(
    connection: &Connection,
    now_ms: i64,
) -> Result<WorkflowInspection, PlatformError> {
    connection.query_row("SELECT
        coalesce(SUM(state='queued'),0),coalesce(SUM(state='running'),0),coalesce(SUM(state='complete'),0),coalesce(SUM(state='errored'),0),
        coalesce(SUM(state_bytes),0),coalesce(SUM(state='running' AND run_lease_until_ms<=?1),0),
        coalesce(SUM(state='waiting'),0),coalesce(SUM(state='paused'),0),coalesce(SUM(state='terminated'),0),
        coalesce(SUM(capability_version=1 AND state IN ('complete','errored','terminated')),0),coalesce(SUM(event_count),0),coalesce(SUM(event_bytes),0),
        coalesce(SUM(CASE WHEN capability_version=1 THEN next_event_seq-1-event_count ELSE 0 END),0),
        (SELECT COUNT(*) FROM workflow_gc_receipts) FROM workflow_instances",[now_ms],|row|Ok(WorkflowInspection {
            queued:row.get(0)?,running:row.get(1)?,complete:row.get(2)?,errored:row.get(3)?,state_bytes:row.get(4)?,expired_runs:row.get(5)?,
            waiting:row.get(6)?,paused:row.get(7)?,terminated:row.get(8)?,retained:row.get(9)?,buffered_events:row.get(10)?,inbox_bytes:row.get(11)?,
            consumed_events:row.get(12)?,gc_receipts:row.get(13)?, ..Default::default()
        })).map_err(sql_error).and_then(|mut summary| {
            connection.query_row("SELECT coalesce(SUM(state='waiting' AND kind IN ('sleep','sleep_until')),0),
                coalesce(SUM(state='waiting' AND kind='wait_event'),0),coalesce(SUM(state='retry_wait'),0),
                coalesce(SUM(state='complete' AND attempt>1),0),coalesce(SUM(error_code='WORKFLOW_STEP_RETRIES_EXHAUSTED'),0),
                coalesce(SUM(error_code='WORKFLOW_STEP_TIMEOUT'),0),coalesce(SUM(error_code='WORKFLOW_EVENT_TIMEOUT'),0)
                FROM workflow_steps WHERE config_sha256 IS NOT NULL",[],|row| {
                    summary.sleeping_steps=row.get(0)?;summary.event_waits=row.get(1)?;summary.retry_waits=row.get(2)?;
                    summary.retried_steps=row.get(3)?;summary.exhausted_steps=row.get(4)?;summary.step_timeouts=row.get(5)?;summary.event_timeouts=row.get(6)?;
                    Ok(())
                }).map_err(sql_error)?;
            Ok(summary)
        })
}

pub(crate) fn workflow_invalid_rows(connection: &Connection) -> Result<u64, PlatformError> {
    let durable: u64 = connection.query_row("SELECT COUNT(*) FROM workflow_instances i
        JOIN workflow_accounting a ON a.id=i.id WHERE
        i.registered_step_count!=a.registered OR i.settled_step_count!=a.settled
        OR i.completed_step_count!=a.completed OR i.event_count!=a.event_count OR i.event_bytes!=a.event_bytes
        OR i.next_wake_at_ms IS NOT a.next_wake
        OR i.state_bytes!=256+length(i.input_json)+coalesce(length(i.output_json),0)+coalesce(length(i.error_json),0)
          +coalesce(length(CAST(i.trigger_cron AS BLOB))+16,0)
          +length(CAST(i.definition_name AS BLOB))+length(CAST(i.external_instance_id AS BLOB))+length(CAST(i.class_name AS BLOB))+a.history_bytes
        OR (i.state='complete' AND a.settled!=a.registered)
        OR (i.state!='running' AND EXISTS(SELECT 1 FROM workflow_steps s WHERE s.instance_id=i.id AND s.state='running'))
        OR (i.state IN ('complete','errored','terminated') AND EXISTS(SELECT 1 FROM workflow_steps s WHERE s.instance_id=i.id AND s.state IN ('pending','delay_pending','waiting','retry_wait')))
        OR (i.state='waiting' AND (a.next_wake IS NULL OR EXISTS(SELECT 1 FROM workflow_steps s WHERE s.instance_id=i.id AND s.state IN ('pending','delay_pending'))))
        OR EXISTS(SELECT 1 FROM workflow_steps s WHERE s.instance_id=i.id AND (s.instance_generation!=i.instance_generation
          OR s.config_sha256 IS NULL OR (s.state='running' AND s.run_token!=i.run_token)
          OR s.ordinal!=(SELECT COUNT(*) FROM workflow_steps p WHERE p.instance_id=i.id AND p.ordinal<s.ordinal)
          OR s.name_count!=1+(SELECT COUNT(*) FROM workflow_steps p WHERE p.instance_id=i.id AND p.kind=s.kind AND p.name=s.name AND p.ordinal<s.ordinal)
          OR s.dependency_count!=(SELECT COUNT(*) FROM workflow_step_dependencies d WHERE d.instance_id=i.id AND d.child_ordinal=s.ordinal)))",
        [],|row|row.get(0)).map_err(sql_error)?;
    Ok(durable)
}
