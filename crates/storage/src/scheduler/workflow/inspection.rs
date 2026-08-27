use super::*;
use open_compute_core::WorkloadSummary;

impl SchedulerStore {
    /// Read a step's durable start timestamp for bounded completion latency metrics.
    pub fn workflow_step_started_at(
        &self,
        id: WorkflowInstanceId,
        ordinal: u32,
    ) -> Result<Option<i64>, PlatformError> {
        self.lock()?
            .query_row(
                "SELECT started_at_ms FROM workflow_steps WHERE instance_id=?1 AND ordinal=?2",
                params![id.to_string(), ordinal],
                |row| row.get(0),
            )
            .optional()
            .map_err(sql_error)
    }

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
        let mut statement = conn.prepare("SELECT id,external_instance_id,version_id,deployment_id,class_name,instance_generation,state,
            completed_step_count,(SELECT COUNT(*) FROM workflow_steps s WHERE s.instance_id=i.id),state_bytes,
            CASE WHEN run_lease_until_ms IS NOT NULL THEN MAX(0,run_lease_until_ms-?5) END,created_at_ms,terminal_at_ms,error_code
            FROM workflow_instances i WHERE account_id=?1 AND definition_id=?2 AND (?3 IS NULL OR id>?3) ORDER BY id LIMIT ?4").map_err(sql_error)?;
        statement
            .query_map(
                params![
                    account.to_string(),
                    definition.to_string(),
                    after.map(|id| id.to_string()),
                    limit,
                    now_ms
                ],
                |row| {
                    let state: String = row.get(6)?;
                    let status = match state.as_str() {
                        "queued" => WorkflowState::Queued,
                        "running" => WorkflowState::Running,
                        "complete" => WorkflowState::Complete,
                        "errored" => WorkflowState::Errored,
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
                    })
                },
            )
            .map_err(sql_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(sql_error)
    }

    /// Aggregate low-cardinality counts without reading retained payloads or exceptions.
    pub fn inspect_workflows(&self, now_ms: i64) -> Result<WorkflowInspection, PlatformError> {
        workflow_inspection_connection(&*self.lock()?, now_ms)
    }

    /// Pool-ready work and earliest persisted lease/backoff deadline.
    pub fn workflow_workload_summary(&self, now_ms: i64) -> Result<WorkloadSummary, PlatformError> {
        self.lock()?.query_row("SELECT coalesce(SUM(state='queued' AND next_run_at_ms<=?1),0),coalesce(SUM(state='running'),0),
            coalesce(SUM(state='running' AND run_lease_until_ms<=?1),0),MIN(next_run_at_ms),
            MIN(coalesce(next_run_at_ms,run_lease_until_ms)) FROM workflow_instances",[now_ms],|row|Ok(WorkloadSummary {
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
                "SELECT ordinal,name,name_count,state,coalesce(length(output_json),0),error_code
            FROM workflow_steps WHERE instance_id=?1 AND ordinal>?2 ORDER BY ordinal LIMIT ?3",
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
        for json in
            std::iter::once(instance.input_json.as_str()).chain(instance.output_json.as_deref())
        {
            if open_compute_core::workflow::canonical_json(
                json,
                ErrorCode::WorkflowInvariantViolation,
            )
            .map_err(|_| error(ErrorCode::WorkflowInvariantViolation))?
                != json
            {
                return Err(error(ErrorCode::WorkflowInvariantViolation));
            }
        }
        let mut statement = conn
            .prepare(
                "SELECT ordinal,name,name_count,config_json,descriptor_sha256,state,
            coalesce(length(output_json),0)+coalesce(length(error_json),0) FROM workflow_steps
            WHERE instance_id=?1 ORDER BY ordinal LIMIT 1025",
            )
            .map_err(sql_error)?;
        let mut rows = statement.query([id.to_string()]).map_err(sql_error)?;
        let mut ordinal = 0;
        let mut completed = 0;
        let mut counts = std::collections::BTreeMap::new();
        let mut bytes = instance.input_json.len() as u64
            + instance.output_json.as_ref().map_or(0, |s| s.len() as u64)
            + u64::from(instance.error.is_some()) * failure_json().len() as u64;
        let mut incomplete = false;
        while let Some(row) = rows.next().map_err(sql_error)? {
            let identity = WorkflowStepIdentity {
                ordinal: row.get(0).map_err(sql_error)?,
                name: row.get(1).map_err(sql_error)?,
                name_count: row.get(2).map_err(sql_error)?,
                config_json: text(row, 3).map_err(sql_error)?,
            };
            let digest: Vec<u8> = row.get(4).map_err(sql_error)?;
            let state: String = row.get(5).map_err(sql_error)?;
            let count = counts.entry(identity.name.clone()).or_insert(0);
            *count += 1;
            if identity.ordinal != ordinal
                || identity.name_count != *count
                || identity.sha256()?.as_slice() != digest
                || incomplete
            {
                return Err(error(ErrorCode::WorkflowInvariantViolation));
            }
            ordinal += 1;
            completed += u32::from(state == "complete");
            incomplete = state != "complete";
            bytes += identity.state_bytes() as u64 + row.get::<_, u64>(6).map_err(sql_error)?;
        }
        if completed != instance.completed_step_count
            || bytes != instance.state_bytes
            || (instance.state == WorkflowState::Complete && (incomplete || ordinal == 0))
        {
            return Err(error(ErrorCode::WorkflowInvariantViolation));
        }
        Ok(())
    }
}

pub(crate) fn workflow_inspection_connection(
    connection: &Connection,
    now_ms: i64,
) -> Result<WorkflowInspection, PlatformError> {
    connection.query_row("SELECT coalesce(SUM(state='queued'),0),coalesce(SUM(state='running'),0),
            coalesce(SUM(state='complete'),0),coalesce(SUM(state='errored'),0),coalesce(SUM(state_bytes),0),
            coalesce(SUM(state='running' AND run_lease_until_ms<=?1),0) FROM workflow_instances",[now_ms],|row|Ok(WorkflowInspection {
                queued: row.get(0)?,running: row.get(1)?,complete: row.get(2)?,errored: row.get(3)?,state_bytes: row.get(4)?,expired_runs: row.get(5)?,
            })).map_err(sql_error)
}

pub(crate) fn workflow_invalid_rows(connection: &Connection) -> Result<u64, PlatformError> {
    connection.query_row("SELECT (SELECT COUNT(*) FROM workflow_instances i WHERE
        (i.state='running') != (i.run_token IS NOT NULL AND i.run_lease_until_ms IS NOT NULL AND i.run_claimed_at_ms IS NOT NULL)
        OR (i.run_token IS NOT NULL AND length(i.run_token)!=32)
        OR i.completed_step_count!=(SELECT COUNT(*) FROM workflow_steps s WHERE s.instance_id=i.id AND s.state='complete')
        OR i.state_bytes!=length(i.input_json)+coalesce(length(i.output_json),0)+coalesce(length(i.error_json),0)
          +coalesce((SELECT SUM(length(CAST(s.name AS BLOB))+length(s.config_json)+50+coalesce(length(s.output_json),0)+coalesce(length(s.error_json),0)) FROM workflow_steps s WHERE s.instance_id=i.id),0)
        OR (i.state='complete' AND (i.completed_step_count=0 OR EXISTS(SELECT 1 FROM workflow_steps s WHERE s.instance_id=i.id AND s.state!='complete')))
        OR (i.state='errored' AND i.error_json!=?1))
        +(SELECT COUNT(*) FROM workflow_steps s LEFT JOIN workflow_instances i ON i.id=s.instance_id
          WHERE i.id IS NULL OR s.instance_generation!=i.instance_generation OR s.attempt!=1
          OR s.ordinal!=(SELECT COUNT(*) FROM workflow_steps p WHERE p.instance_id=s.instance_id AND p.ordinal<s.ordinal)
          OR s.name_count!=1+(SELECT COUNT(*) FROM workflow_steps p WHERE p.instance_id=s.instance_id AND p.name=s.name AND p.ordinal<s.ordinal)
          OR (s.state='failed' AND s.error_json!=?1)
          OR (s.state='running' AND i.state='running' AND s.run_token!=i.run_token))",
        [failure_json().as_bytes()],|row|row.get(0)).map_err(sql_error)
}
