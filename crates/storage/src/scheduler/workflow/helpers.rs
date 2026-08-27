use super::*;

pub(super) fn error(code: ErrorCode) -> PlatformError {
    PlatformError::new(code, "Workflow operation failed")
}
// `Result::map_err` transfers the driver error; no raw SQL detail escapes this boundary.
#[allow(clippy::needless_pass_by_value)]
pub(super) fn sql_error(err: rusqlite::Error) -> PlatformError {
    let code = match &err {
        rusqlite::Error::SqliteFailure(info, _)
            if matches!(
                info.code,
                rusqlite::ErrorCode::DatabaseBusy
                    | rusqlite::ErrorCode::DatabaseLocked
                    | rusqlite::ErrorCode::DiskFull
            ) =>
        {
            ErrorCode::WorkflowRuntimeUnavailable
        }
        _ => ErrorCode::WorkflowInvariantViolation,
    };
    error(code)
}
pub(super) fn token() -> Result<WorkflowToken, PlatformError> {
    let mut value = [0; 32];
    rand::rngs::OsRng
        .try_fill_bytes(&mut value)
        .map_err(|_| error(ErrorCode::WorkflowRuntimeUnavailable))?;
    Ok(WorkflowToken::from_bytes(value))
}
pub(super) fn text(row: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<String> {
    String::from_utf8(row.get(index)?).map_err(|_| rusqlite::Error::InvalidQuery)
}
pub(super) fn parse<T: std::str::FromStr>(
    row: &rusqlite::Row<'_>,
    index: usize,
) -> rusqlite::Result<T> {
    row.get::<_, String>(index)?
        .parse()
        .map_err(|_| rusqlite::Error::InvalidQuery)
}
pub(super) fn failure_code(
    row: &rusqlite::Row<'_>,
    index: usize,
) -> rusqlite::Result<Option<String>> {
    let code: Option<String> = row.get(index)?;
    if let Some(code) = &code {
        open_compute_core::workflow::terminal_error_code(code)
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
    }
    Ok(code)
}
fn digest(row: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<[u8; 32]> {
    row.get::<_, Vec<u8>>(index)?
        .try_into()
        .map_err(|_| rusqlite::Error::InvalidQuery)
}
pub(super) const INSTANCE_SELECT: &str = "SELECT account_id,definition_id,definition_name,version_id,worker_id,deployment_id,
    worker_code_sha256,class_name,loader_schema_version,capability_version,descriptor_sha256,id,external_instance_id,
    instance_generation,creation_nonce,created_at_ms,updated_at_ms,state,input_json,output_json,error_json,error_code,
    run_token,run_lease_until_ms,next_run_at_ms,completed_step_count,state_bytes,terminal_at_ms FROM workflow_instances";
pub(super) fn instance_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<WorkflowInstanceRecord> {
    let state = match row.get::<_, String>(17)?.as_str() {
        "queued" => WorkflowState::Queued,
        "running" => WorkflowState::Running,
        "complete" => WorkflowState::Complete,
        "errored" => WorkflowState::Errored,
        _ => return Err(rusqlite::Error::InvalidQuery),
    };
    Ok(WorkflowInstanceRecord {
        identity: WorkflowInstanceIdentity {
            target: WorkflowTarget {
                account_id: parse(row, 0)?,
                definition_id: parse(row, 1)?,
                definition_name: row.get(2)?,
                version_id: parse(row, 3)?,
                worker_id: parse(row, 4)?,
                deployment_id: parse(row, 5)?,
                worker_code_sha256: digest(row, 6)?,
                class_name: row.get(7)?,
                loader_schema_version: row.get(8)?,
                capability_version: row.get(9)?,
                descriptor_sha256: digest(row, 10)?,
            },
            instance_id: parse(row, 11)?,
            external_instance_id: row.get(12)?,
            instance_generation: row.get(13)?,
            creation_nonce: WorkflowToken::from_bytes(digest(row, 14)?),
            created_at_ms: row.get(15)?,
        },
        updated_at_ms: row.get(16)?,
        state,
        input_json: text(row, 18)?,
        output_json: row
            .get::<_, Option<Vec<u8>>>(19)?
            .map(|bytes| String::from_utf8(bytes).map_err(|_| rusqlite::Error::InvalidQuery))
            .transpose()?,
        error: row
            .get::<_, Option<Vec<u8>>>(20)?
            .map(|bytes| {
                let failure: WorkflowFailure =
                    serde_json::from_slice(&bytes).map_err(|_| rusqlite::Error::InvalidQuery)?;
                if failure != WorkflowFailure::default() {
                    return Err(rusqlite::Error::InvalidQuery);
                }
                Ok(failure)
            })
            .transpose()?,
        error_code: failure_code(row, 21)?,
        run_token: row
            .get::<_, Option<Vec<u8>>>(22)?
            .map(|bytes| {
                bytes
                    .try_into()
                    .map(WorkflowToken::from_bytes)
                    .map_err(|_| rusqlite::Error::InvalidQuery)
            })
            .transpose()?,
        run_lease_until_ms: row.get(23)?,
        next_run_at_ms: row.get(24)?,
        completed_step_count: row.get(25)?,
        state_bytes: row.get(26)?,
        terminal_at_ms: row.get(27)?,
    })
}

pub(super) fn running(
    conn: &Connection,
    fence: &WorkflowFence,
    now_ms: i64,
) -> Result<WorkflowInstanceRecord, PlatformError> {
    conn.query_row(
        &format!(
            "{INSTANCE_SELECT} WHERE id=?1 AND instance_generation=?2 AND run_token=?3
        AND state='running' AND run_lease_until_ms>?4"
        ),
        params![
            fence.instance_id.to_string(),
            fence.instance_generation,
            fence.run_token.as_bytes().as_slice(),
            now_ms
        ],
        instance_row,
    )
    .optional()
    .map_err(sql_error)?
    .ok_or_else(|| error(ErrorCode::WorkflowRunStale))
}

pub(super) fn capacity(
    conn: &Connection,
    account: open_compute_core::AccountId,
    retained: u64,
    extra: usize,
    terminal: bool,
    limits: &WorkflowsConfig,
) -> Result<(), PlatformError> {
    // Reserve enough room for a sanitized terminal failure for every admitted live run.
    let (total, active): (u64, u64) = conn
        .query_row(
            "SELECT coalesce(SUM(state_bytes),0),coalesce(SUM(state IN ('queued','running')),0)
        FROM workflow_instances WHERE account_id=?1",
            [account.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(sql_error)?;
    let extra = extra as u64;
    let reserve = failure_json().len() as u64;
    let instance_reserve = if terminal || retained == 0 {
        0
    } else {
        reserve
    };
    let active = active.saturating_sub(u64::from(terminal));
    if retained
        .saturating_add(extra)
        .saturating_add(instance_reserve)
        > limits.max_state_bytes
        || total
            .saturating_add(extra)
            .saturating_add(active.saturating_mul(reserve))
            > limits.max_account_state_bytes
    {
        return Err(error(ErrorCode::WorkflowStateQuotaExceeded));
    }
    Ok(())
}

pub(super) fn bounded(limit: u32) -> Result<(), PlatformError> {
    if limit == 0 || limit > 1000 {
        return Err(error(ErrorCode::LimitInvalid));
    }
    Ok(())
}

pub(super) fn failure_json() -> &'static str {
    r#"{"name":"Error","message":"Workflow execution failed"}"#
}

pub(super) fn heartbeat(
    conn: &Connection,
    fence: &WorkflowFence,
    now_ms: i64,
    limits: &WorkflowsConfig,
) -> Result<(), PlatformError> {
    let changed = conn.execute("UPDATE workflow_instances SET run_lease_until_ms=max(run_lease_until_ms,?4),updated_at_ms=max(updated_at_ms,?5)
        WHERE id=?1 AND instance_generation=?2 AND run_token=?3 AND state='running' AND run_lease_until_ms>?5",
        params![fence.instance_id.to_string(),fence.instance_generation,fence.run_token.as_bytes().as_slice(),deadline(now_ms,limits.lease_ms)?,now_ms])
        .map_err(sql_error)?;
    if changed != 1 {
        return Err(error(ErrorCode::WorkflowRunStale));
    }
    Ok(())
}

pub(super) fn deadline(now_ms: i64, duration: u64) -> Result<i64, PlatformError> {
    now_ms
        .checked_add(i64::try_from(duration).map_err(|_| error(ErrorCode::LimitInvalid))?)
        .ok_or_else(|| error(ErrorCode::LimitInvalid))
}

impl WorkflowStepIdentity {
    /// Validate the supported sequential identity and hash its canonical descriptor.
    pub fn sha256(&self) -> Result<[u8; 32], PlatformError> {
        if self.config_json != "null" {
            return Err(error(ErrorCode::WorkflowStepConfigUnsupported));
        }
        if self.name.is_empty() || self.name.len() > 256 || self.name_count == 0 {
            return Err(error(ErrorCode::WorkflowSerializationUnsupported));
        }
        if self.ordinal >= 1024 {
            return Err(error(ErrorCode::WorkflowStepLimitExceeded));
        }
        let descriptor = serde_json::json!({"kind":"do","ordinal":self.ordinal,"name":self.name,
            "nameCount":self.name_count,"config":null});
        Ok(Sha256::digest(
            serde_json::to_vec(&descriptor)
                .map_err(|_| error(ErrorCode::WorkflowInvariantViolation))?,
        )
        .into())
    }
    /// Logical descriptor bytes accounted by the schema triggers.
    #[must_use]
    pub fn state_bytes(&self) -> usize {
        self.name.len() + self.config_json.len() + 50
    }
}
