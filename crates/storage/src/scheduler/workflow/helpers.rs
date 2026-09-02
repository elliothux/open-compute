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
pub(super) fn text(
    row: &rusqlite::Row<'_>,
    index: impl rusqlite::RowIndex,
) -> rusqlite::Result<String> {
    String::from_utf8(row.get(index)?).map_err(|_| rusqlite::Error::InvalidQuery)
}
pub(super) fn parse<T: std::str::FromStr>(
    row: &rusqlite::Row<'_>,
    index: impl rusqlite::RowIndex,
) -> rusqlite::Result<T> {
    row.get::<_, String>(index)?
        .parse()
        .map_err(|_| rusqlite::Error::InvalidQuery)
}
pub(super) fn failure_code(
    row: &rusqlite::Row<'_>,
    index: impl rusqlite::RowIndex,
) -> rusqlite::Result<Option<String>> {
    let code: Option<String> = row.get(index)?;
    if let Some(code) = &code {
        open_compute_core::workflow::terminal_error_code(code)
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
    }
    Ok(code)
}
pub(super) fn digest(
    row: &rusqlite::Row<'_>,
    index: impl rusqlite::RowIndex,
) -> rusqlite::Result<[u8; 32]> {
    row.get::<_, Vec<u8>>(index)?
        .try_into()
        .map_err(|_| rusqlite::Error::InvalidQuery)
}
pub(super) const INSTANCE_SELECT: &str = "SELECT * FROM workflow_instances";
pub(super) fn instance_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<WorkflowInstanceRecord> {
    let capability: i64 = row.get("capability_version")?;
    if capability != 1 {
        return Err(rusqlite::Error::InvalidQuery);
    }
    let state = match row.get::<_, String>("state")?.as_str() {
        "queued" => WorkflowState::Queued,
        "running" => WorkflowState::Running,
        "complete" => WorkflowState::Complete,
        "errored" => WorkflowState::Errored,
        "waiting" => WorkflowState::Waiting,
        "paused" => WorkflowState::Paused,
        "terminated" => WorkflowState::Terminated,
        _ => return Err(rusqlite::Error::InvalidQuery),
    };
    Ok(WorkflowInstanceRecord {
        identity: WorkflowInstanceIdentity {
            target: WorkflowTarget {
                account_id: parse(row, "account_id")?,
                definition_id: parse(row, "definition_id")?,
                definition_name: row.get("definition_name")?,
                workflow_version_id: parse(row, "workflow_version_id")?,
                worker_id: parse(row, "worker_id")?,
                worker_version_id: parse(row, "worker_version_id")?,
                worker_code_sha256: digest(row, "worker_code_sha256")?,
                class_name: row.get("class_name")?,
                loader_schema_version: row.get("loader_schema_version")?,
                capability_version: capability,
                descriptor_sha256: digest(row, "descriptor_sha256")?,
            },
            instance_id: parse(row, "id")?,
            external_instance_id: row.get("external_instance_id")?,
            instance_generation: row.get("instance_generation")?,
            creation_nonce: WorkflowToken::from_bytes(digest(row, "creation_nonce")?),
            creation_operation_id: parse(row, "creation_operation_id")?,
            creation_batch_id: parse(row, "creation_batch_id")?,
            created_at_ms: row.get("created_at_ms")?,
            schedule: match (
                row.get::<_, Option<String>>("trigger_cron")?,
                row.get::<_, Option<i64>>("trigger_scheduled_time_ms")?,
            ) {
                (Some(cron), Some(scheduled_time)) => {
                    Some(open_compute_core::WorkflowCronSchedule {
                        cron,
                        scheduled_time,
                    })
                }
                (None, None) => None,
                _ => return Err(rusqlite::Error::InvalidQuery),
            },
        },
        updated_at_ms: row.get("updated_at_ms")?,
        state,
        input_json: text(row, "input_json")?,
        output_json: row
            .get::<_, Option<Vec<u8>>>("output_json")?
            .map(|bytes| String::from_utf8(bytes).map_err(|_| rusqlite::Error::InvalidQuery))
            .transpose()?,
        error: row
            .get::<_, Option<Vec<u8>>>("error_json")?
            .map(|bytes| {
                let failure: WorkflowFailure =
                    serde_json::from_slice(&bytes).map_err(|_| rusqlite::Error::InvalidQuery)?;
                if failure != WorkflowFailure::default() {
                    return Err(rusqlite::Error::InvalidQuery);
                }
                Ok(failure)
            })
            .transpose()?,
        error_code: failure_code(row, "error_code")?,
        run_token: row
            .get::<_, Option<Vec<u8>>>("run_token")?
            .map(|bytes| {
                bytes
                    .try_into()
                    .map(WorkflowToken::from_bytes)
                    .map_err(|_| rusqlite::Error::InvalidQuery)
            })
            .transpose()?,
        run_lease_until_ms: row.get("run_lease_until_ms")?,
        next_run_at_ms: row.get("next_run_at_ms")?,
        completed_step_count: row.get("completed_step_count")?,
        state_bytes: row.get("state_bytes")?,
        terminal_at_ms: row.get("terminal_at_ms")?,
        durable: durable_state(row)?,
    })
}

pub(super) fn durable_state(row: &rusqlite::Row<'_>) -> rusqlite::Result<WorkflowDurableState> {
    let retention = open_compute_core::workflow::WorkflowRetention {
        success_retention_ms: row.get("success_retention_ms")?,
        error_retention_ms: row.get("error_retention_ms")?,
    };
    retention
        .validate()
        .map_err(|_| rusqlite::Error::InvalidQuery)?;
    Ok(WorkflowDurableState {
        pause_requested: row.get("pause_requested")?,
        yield_requested: row.get("yield_requested")?,
        rollback_requested: row.get("rollback_requested")?,
        next_wake_at_ms: row.get("next_wake_at_ms")?,
        registered_step_count: row.get("registered_step_count")?,
        settled_step_count: row.get("settled_step_count")?,
        retention,
        expires_at_ms: row.get("expires_at_ms")?,
        last_restart_operation_id: row
            .get::<_, Option<String>>("last_restart_operation_id")?
            .map(|value| value.parse().map_err(|_| rusqlite::Error::InvalidQuery))
            .transpose()?,
        event_count: row.get("event_count")?,
        event_bytes: row.get("event_bytes")?,
        next_event_seq: row.get("next_event_seq")?,
        has_activated: row.get("has_activated")?,
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
    let (total, active) = account_capacity(conn, account)?;
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

pub(super) fn account_capacity(
    conn: &Connection,
    account: open_compute_core::AccountId,
) -> Result<(u64, u64), PlatformError> {
    conn.query_row(
        "SELECT coalesce(SUM(state_bytes),0),
        coalesce(SUM(state IN ('queued','running','waiting','paused')),0)
        +(SELECT COUNT(*) FROM workflow_steps s JOIN workflow_instances i ON i.id=s.instance_id
          WHERE i.account_id=?1 AND i.capability_version=1 AND s.kind IN ('do','wait_event')
          AND s.state IN ('pending','running','delay_pending','waiting'))
        FROM workflow_instances WHERE account_id=?1",
        [account.to_string()],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )
    .map_err(sql_error)
}

/// Admission charges pending failures as well as bytes; settlements may use an existing reservation
/// even after capacity is lowered, but may not introduce new growth beyond the current policy.
pub(super) fn capacity_change(
    conn: &Connection,
    instance: &WorkflowInstanceRecord,
    extra_bytes: i64,
    extra_reservations: i64,
    limits: &WorkflowsConfig,
) -> Result<(), PlatformError> {
    let (bytes,reservations):(u64,u64)=conn.query_row("SELECT state_bytes,
        (state IN ('queued','running','waiting','paused'))
        +(SELECT COUNT(*) FROM workflow_steps s WHERE s.instance_id=i.id AND s.kind IN ('do','wait_event')
          AND s.state IN ('pending','running','delay_pending','waiting')) FROM workflow_instances i WHERE id=?1 AND capability_version=1",
        [instance.identity.instance_id.to_string()],|row|Ok((row.get(0)?,row.get(1)?))).map_err(sql_error)?;
    let (account_bytes, account_reservations) =
        account_capacity(conn, instance.identity.target.account_id)?;
    let reserve = failure_json().len() as i128;
    let change = i128::from(extra_bytes) + i128::from(extra_reservations) * reserve;
    for (current, limit) in [
        (
            i128::from(bytes) + i128::from(reservations) * reserve,
            limits.max_state_bytes,
        ),
        (
            i128::from(account_bytes) + i128::from(account_reservations) * reserve,
            limits.max_account_state_bytes,
        ),
    ] {
        if current + change < 0 {
            return Err(error(ErrorCode::WorkflowInvariantViolation));
        }
        if change > 0 && current + change > i128::from(limit) {
            return Err(error(ErrorCode::WorkflowStateQuotaExceeded));
        }
    }
    Ok(())
}

pub(super) fn durable_deadline(now_ms: i64, duration: u64) -> Result<i64, PlatformError> {
    let value = deadline(now_ms, duration)?;
    if value.unsigned_abs() > open_compute_core::workflow::WORKFLOW_MAX_SAFE_INTEGER {
        return Err(error(ErrorCode::WorkflowDurationInvalid));
    }
    Ok(value)
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

pub(super) fn initial_state_bytes(
    identity: &WorkflowInstanceIdentity,
    input_bytes: usize,
) -> usize {
    input_bytes
        + open_compute_core::workflow::WORKFLOW_INSTANCE_BYTES
        + identity.target.definition_name.len()
        + identity.external_instance_id.len()
        + identity.target.class_name.len()
        + identity
            .schedule
            .as_ref()
            .map_or(0, |value| value.cron.len() + 16)
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
