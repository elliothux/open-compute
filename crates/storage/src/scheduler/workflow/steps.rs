use super::runs::{terminal_code, terminal_error};
use super::*;

struct StepRow {
    descriptor: [u8; 32],
    state: String,
    run_token: Option<Vec<u8>>,
    step_token: Option<Vec<u8>>,
    output: Option<String>,
    error_json: Option<Vec<u8>>,
    error_code: Option<String>,
}

impl SchedulerStore {
    /// Replay an immutable result or durably grant one sequential callback execution.
    /// Descriptor and capacity violations commit a permanent instance error before replying.
    pub fn claim_workflow_step(
        &self,
        fence: &WorkflowFence,
        identity: &WorkflowStepIdentity,
        now_ms: i64,
        limits: &WorkflowsConfig,
    ) -> Result<WorkflowStepGrant, PlatformError> {
        limits.validate()?;
        let mut conn = self.lock()?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sql_error)?;
        let instance = running(&tx, fence, now_ms)?;
        let result = claim(&tx, fence, identity, &instance, now_ms, limits);
        match result {
            Ok(grant) => {
                heartbeat(&tx, fence, now_ms, limits)?;
                tx.commit().map_err(sql_error)?;
                Ok(grant)
            }
            Err(err) if terminal_code(err.code()).is_ok() => {
                terminal_error(&tx, fence, err.code(), now_ms)?;
                tx.commit().map_err(sql_error)?;
                Err(err)
            }
            Err(err) => Err(err),
        }
    }

    /// Commit canonical step output under both exact run and step tokens before returning a value.
    pub fn complete_workflow_step(
        &self,
        fence: &WorkflowFence,
        ordinal: u32,
        step_token: &WorkflowToken,
        output: &str,
        now_ms: i64,
        limits: &WorkflowsConfig,
    ) -> Result<(), PlatformError> {
        limits.validate()?;
        let mut conn = self.lock()?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sql_error)?;
        let instance = running(&tx, fence, now_ms)?;
        verify_step(&tx, fence, ordinal, step_token)?;
        let output =
            open_compute_core::workflow::canonical_json(output, ErrorCode::WorkflowResultTooLarge)
                .and_then(|output| {
                    capacity(
                        &tx,
                        instance.identity.target.account_id,
                        instance.state_bytes,
                        output.len(),
                        false,
                        limits,
                    )?;
                    Ok(output)
                });
        match output {
            Ok(output) => {
                tx.execute("UPDATE workflow_steps SET state='complete',output_json=?5,run_token=NULL,step_token=NULL,
                    completed_at_ms=?6,updated_at_ms=?6 WHERE instance_id=?1 AND instance_generation=?2 AND ordinal=?3
                    AND state='running' AND step_token=?4 AND run_token=?7",params![fence.instance_id.to_string(),fence.instance_generation,
                        ordinal,step_token.as_bytes().as_slice(),output.as_bytes(),now_ms,fence.run_token.as_bytes().as_slice()]).map_err(sql_error)?;
                heartbeat(&tx, fence, now_ms, limits)?;
                tx.commit().map_err(sql_error)
            }
            Err(err) => {
                terminal_error(&tx, fence, err.code(), now_ms)?;
                tx.commit().map_err(sql_error)?;
                Err(err)
            }
        }
    }

    /// Commit a known callback failure without accepting tenant exception text or stack.
    pub fn fail_workflow_step(
        &self,
        fence: &WorkflowFence,
        ordinal: u32,
        step_token: &WorkflowToken,
        code: ErrorCode,
        now_ms: i64,
        limits: &WorkflowsConfig,
    ) -> Result<(), PlatformError> {
        limits.validate()?;
        terminal_code(code)?;
        let mut conn = self.lock()?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sql_error)?;
        let instance = running(&tx, fence, now_ms)?;
        verify_step(&tx, fence, ordinal, step_token)?;
        if let Err(err) = capacity(
            &tx,
            instance.identity.target.account_id,
            instance.state_bytes,
            failure_json().len(),
            false,
            limits,
        ) {
            if err.code() == ErrorCode::WorkflowStateQuotaExceeded {
                terminal_error(&tx, fence, err.code(), now_ms)?;
                tx.commit().map_err(sql_error)?;
            }
            return Err(err);
        }
        tx.execute("UPDATE workflow_steps SET state='failed',error_json=?5,error_code=?6,run_token=NULL,step_token=NULL,
            completed_at_ms=?7,updated_at_ms=?7 WHERE instance_id=?1 AND instance_generation=?2 AND ordinal=?3
            AND state='running' AND step_token=?4 AND run_token=?8",params![fence.instance_id.to_string(),fence.instance_generation,
                ordinal,step_token.as_bytes().as_slice(),failure_json().as_bytes(),code.as_str(),now_ms,fence.run_token.as_bytes().as_slice()]).map_err(sql_error)?;
        heartbeat(&tx, fence, now_ms, limits)?;
        tx.commit().map_err(sql_error)
    }
}

fn verify_step(
    conn: &Connection,
    fence: &WorkflowFence,
    ordinal: u32,
    step_token: &WorkflowToken,
) -> Result<(), PlatformError> {
    let exact: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM workflow_steps WHERE instance_id=?1 AND instance_generation=?2
        AND ordinal=?3 AND state='running' AND run_token=?4 AND step_token=?5)",params![fence.instance_id.to_string(),fence.instance_generation,
            ordinal,fence.run_token.as_bytes().as_slice(),step_token.as_bytes().as_slice()],|row|row.get(0)).map_err(sql_error)?;
    if !exact {
        return Err(error(ErrorCode::WorkflowStepStale));
    }
    Ok(())
}

fn claim(
    conn: &Connection,
    fence: &WorkflowFence,
    identity: &WorkflowStepIdentity,
    instance: &WorkflowInstanceRecord,
    now_ms: i64,
    limits: &WorkflowsConfig,
) -> Result<WorkflowStepGrant, PlatformError> {
    let descriptor = identity.sha256()?;
    if identity.ordinal >= limits.max_steps {
        return Err(error(ErrorCode::WorkflowStepLimitExceeded));
    }
    let existing = conn
        .query_row(
            "SELECT descriptor_sha256,state,run_token,step_token,output_json,error_json,error_code FROM workflow_steps
        WHERE instance_id=?1 AND instance_generation=?2 AND ordinal=?3",
            params![
                fence.instance_id.to_string(),
                fence.instance_generation,
                identity.ordinal
            ],
            |row| {
                Ok(StepRow {
                    descriptor: row
                        .get::<_, Vec<u8>>(0)?
                        .try_into()
                        .map_err(|_| rusqlite::Error::InvalidQuery)?,
                    state: row.get(1)?,
                    error_json:row.get(5)?,
                    error_code:row.get(6)?,
                    run_token: row.get(2)?,
                    step_token: row.get(3)?,
                    output: row
                        .get::<_, Option<Vec<u8>>>(4)?
                        .map(|bytes| {
                            String::from_utf8(bytes).map_err(|_| rusqlite::Error::InvalidQuery)
                        })
                        .transpose()?,
                })
            },
        )
        .optional()
        .map_err(sql_error)?;
    if let Some(row) = &existing {
        if row.descriptor != descriptor {
            return Err(error(ErrorCode::WorkflowNonDeterministic));
        }
        match row.state.as_str() {
            "complete" => {
                let output = row
                    .output
                    .as_ref()
                    .ok_or_else(|| error(ErrorCode::WorkflowInvariantViolation))?;
                if open_compute_core::workflow::canonical_json(
                    output,
                    ErrorCode::WorkflowResultTooLarge,
                )? != *output
                {
                    return Err(error(ErrorCode::WorkflowInvariantViolation));
                }
                return Ok(WorkflowStepGrant::Complete {
                    output_json: output.clone(),
                });
            }
            "failed" => {
                if row.error_json.as_deref() != Some(failure_json().as_bytes()) {
                    return Err(error(ErrorCode::WorkflowInvariantViolation));
                }
                let code = row
                    .error_code
                    .as_deref()
                    .ok_or_else(|| error(ErrorCode::WorkflowInvariantViolation))?;
                open_compute_core::workflow::terminal_error_code(code)
                    .map_err(|_| error(ErrorCode::WorkflowInvariantViolation))?;
                return Ok(WorkflowStepGrant::Failed {
                    error: WorkflowFailure::default(),
                    error_code: code.into(),
                });
            }
            "running" => {
                if row.run_token.as_deref() != Some(fence.run_token.as_bytes().as_slice()) {
                    return Err(error(ErrorCode::WorkflowStepStale));
                }
                let bytes = row
                    .step_token
                    .clone()
                    .ok_or_else(|| error(ErrorCode::WorkflowInvariantViolation))?
                    .try_into()
                    .map_err(|_| error(ErrorCode::WorkflowInvariantViolation))?;
                return Ok(WorkflowStepGrant::Run {
                    step_token: WorkflowToken::from_bytes(bytes),
                });
            }
            "pending" => {}
            _ => return Err(error(ErrorCode::WorkflowInvariantViolation)),
        }
    }
    if identity.ordinal != instance.completed_step_count {
        return Err(error(ErrorCode::WorkflowNonDeterministic));
    }
    let step_token = token()?;
    if existing.is_some() {
        conn.execute(
            "UPDATE workflow_steps SET state='running',run_token=?4,step_token=?5,updated_at_ms=?6
            WHERE instance_id=?1 AND instance_generation=?2 AND ordinal=?3 AND state='pending'",
            params![
                fence.instance_id.to_string(),
                fence.instance_generation,
                identity.ordinal,
                fence.run_token.as_bytes().as_slice(),
                step_token.as_bytes().as_slice(),
                now_ms
            ],
        )
        .map_err(sql_error)?;
    } else {
        let (unfinished,count): (bool,u32) = conn.query_row("SELECT EXISTS(SELECT 1 FROM workflow_steps WHERE instance_id=?1 AND state!='complete'),
            (SELECT COUNT(*) FROM workflow_steps WHERE instance_id=?1 AND name=?2)",params![fence.instance_id.to_string(),identity.name],
            |row|Ok((row.get(0)?,row.get(1)?))).map_err(sql_error)?;
        if unfinished {
            return Err(error(ErrorCode::WorkflowParallelStepUnsupported));
        }
        if identity.name_count != count + 1 {
            return Err(error(ErrorCode::WorkflowNonDeterministic));
        }
        capacity(
            conn,
            instance.identity.target.account_id,
            instance.state_bytes,
            identity.state_bytes(),
            false,
            limits,
        )?;
        conn.execute("INSERT INTO workflow_steps(instance_id,instance_generation,ordinal,name,name_count,kind,config_json,
            descriptor_sha256,state,attempt,run_token,step_token,started_at_ms,updated_at_ms)
            VALUES(?1,?2,?3,?4,?5,'do',?6,?7,'running',1,?8,?9,?10,?10)",params![fence.instance_id.to_string(),fence.instance_generation,
                identity.ordinal,identity.name,identity.name_count,identity.config_json.as_bytes(),descriptor.as_slice(),
                fence.run_token.as_bytes().as_slice(),step_token.as_bytes().as_slice(),now_ms]).map_err(sql_error)?;
    }
    Ok(WorkflowStepGrant::Run { step_token })
}
