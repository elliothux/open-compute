//! Private step protocol and validated persisted step metadata.

use super::*;
use open_compute_core::workflow::{
    WorkflowDurableConfig, WorkflowStepConfig, WorkflowStepDescriptor, WorkflowStepKind,
};
use serde::{Deserialize, Serialize};

/// Private callback grant; completed outputs are fetched separately to bound batch replies.
#[derive(Debug, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum WorkflowStepGrant {
    /// Execute this exact business attempt under its private token and remaining deadline.
    Run {
        /// Random step token, visible only to the system-isolate controller.
        #[serde(rename = "stepToken")]
        step_token: WorkflowToken,
        /// One-based business attempt, unchanged by Unknown recovery.
        attempt: u32,
        /// Trusted remaining wall-clock duration for the system timeout tracker.
        #[serde(rename = "remainingMs")]
        remaining_ms: u64,
        /// Fully resolved frozen callback policy.
        config: WorkflowStepConfig,
    },
    /// Immutable output is available through the result operation.
    Complete,
    /// Immutable sanitized failure is available through the result operation.
    Failed,
    /// Durable waiting or a budget boundary requires a drained yield.
    Suspended,
}

/// Committed result of one step, never an optimistic callback return.
#[derive(Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum WorkflowStepResult {
    /// Canonical callback/event output, absent for sleep's void result.
    Complete {
        /// Retained JSON bytes; event payload depth is independent of its envelope.
        #[serde(rename = "outputJson", skip_serializing_if = "Option::is_none")]
        output_json: Option<String>,
    },
    /// A stable failure, with no tenant message or stack.
    Failed {
        /// Frozen error category.
        code: String,
    },
    /// The instance must release the activation after sibling callbacks drain.
    Suspended,
}

impl std::fmt::Debug for WorkflowStepResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Complete { .. } => "Complete([REDACTED])",
            Self::Failed { .. } => "Failed",
            Self::Suspended => "Suspended",
        })
    }
}

/// Exact private callback attempt identity, combined with the owning run fence.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkflowStepAttempt {
    /// Immutable step ordinal.
    pub ordinal: u32,
    /// One-based persisted business attempt.
    pub attempt: u32,
    /// Private token granted by the scheduler, not supplied by tenant code.
    pub step_token: WorkflowToken,
}

/// Trusted callback report. Raw exception text is deliberately not representable.
#[derive(Clone, Copy)]
pub enum WorkflowStepOutcome<'a> {
    /// Serialize and commit a supported JSON result before resolving the callback.
    Success(&'a str),
    /// Sanitized known callback or protocol failure.
    Failure(ErrorCode),
    /// Trusted host timer observed the persisted attempt deadline.
    Timeout,
}
impl std::fmt::Debug for WorkflowStepOutcome<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Success(_) => "Success([REDACTED])",
            Self::Failure(_) => "Failure",
            Self::Timeout => "Timeout",
        })
    }
}

pub(super) struct DurableStep {
    pub descriptor: WorkflowStepDescriptor,
    pub state: String,
    pub attempt: u32,
    pub run_token: Option<WorkflowToken>,
    pub step_token: Option<WorkflowToken>,
    pub deadline: Option<i64>,
    pub due: Option<i64>,
    pub ceiling: Option<i64>,
    pub output: Option<String>,
    pub failure: Option<String>,
}

pub(super) fn read_step(
    conn: &Connection,
    id: WorkflowInstanceId,
    generation: i64,
    ordinal: u32,
) -> Result<Option<DurableStep>, PlatformError> {
    let mut edges = conn.prepare("SELECT parent_ordinal FROM workflow_step_dependencies
        WHERE instance_id=?1 AND instance_generation=?2 AND child_ordinal=?3 ORDER BY parent_ordinal LIMIT 17").map_err(sql_error)?;
    let dependencies = edges
        .query_map(params![id.to_string(), generation, ordinal], |row| {
            row.get(0)
        })
        .map_err(sql_error)?
        .collect::<Result<Vec<u32>, _>>()
        .map_err(sql_error)?;
    let mut statement = conn.prepare("SELECT * FROM workflow_steps WHERE instance_id=?1 AND instance_generation=?2 AND ordinal=?3").map_err(sql_error)?;
    let mut rows = statement
        .query(params![id.to_string(), generation, ordinal])
        .map_err(sql_error)?;
    let Some(row) = rows.next().map_err(sql_error)? else {
        return Ok(None);
    };
    let output = row
        .get::<_, Option<Vec<u8>>>("output_json")
        .map_err(sql_error)?
        .map(|value| {
            String::from_utf8(value).map_err(|_| error(ErrorCode::WorkflowInvariantViolation))
        })
        .transpose()?;
    let failure: Option<Vec<u8>> = row.get("error_json").map_err(sql_error)?;
    let code = failure_code(row, "error_code").map_err(sql_error)?;
    if failure
        .as_deref()
        .is_some_and(|value| value != failure_json().as_bytes())
        || failure.is_some() != code.is_some()
    {
        return Err(error(ErrorCode::WorkflowInvariantViolation));
    }
    Ok(Some(DurableStep {
        descriptor: descriptor(row, dependencies)?,
        state: row.get("state").map_err(sql_error)?,
        attempt: row.get("attempt").map_err(sql_error)?,
        run_token: row_token(row, "run_token")?,
        step_token: row_token(row, "step_token")?,
        deadline: row.get("attempt_deadline_at_ms").map_err(sql_error)?,
        due: row.get("due_at_ms").map_err(sql_error)?,
        ceiling: row.get("event_buffer_ceiling").map_err(sql_error)?,
        output,
        failure: code,
    }))
}

fn row_token(row: &rusqlite::Row<'_>, field: &str) -> Result<Option<WorkflowToken>, PlatformError> {
    row.get::<_, Option<Vec<u8>>>(field)
        .map_err(sql_error)?
        .map(|bytes| {
            bytes
                .try_into()
                .map(WorkflowToken::from_bytes)
                .map_err(|_| error(ErrorCode::WorkflowInvariantViolation))
        })
        .transpose()
}

pub(super) fn descriptor(
    row: &rusqlite::Row<'_>,
    dependencies: Vec<u32>,
) -> Result<WorkflowStepDescriptor, PlatformError> {
    let kind = match row.get::<_, String>("kind").map_err(sql_error)?.as_str() {
        "do" => WorkflowStepKind::Do,
        "sleep" => WorkflowStepKind::Sleep,
        "sleep_until" => WorkflowStepKind::SleepUntil,
        "wait_event" => WorkflowStepKind::WaitEvent,
        _ => return Err(error(ErrorCode::WorkflowInvariantViolation)),
    };
    let config_json = text(row, "config_json").map_err(sql_error)?;
    let descriptor = WorkflowStepDescriptor {
        ordinal: row.get("ordinal").map_err(sql_error)?,
        name: row.get("name").map_err(sql_error)?,
        name_count: row.get("name_count").map_err(sql_error)?,
        config: WorkflowDurableConfig::from_canonical(kind, &config_json)?,
        dependencies,
        batch_first_ordinal: row.get("batch_first_ordinal").map_err(sql_error)?,
        batch_size: row.get("batch_size").map_err(sql_error)?,
    };
    let config_hash: Vec<u8> = row.get("config_sha256").map_err(sql_error)?;
    let descriptor_hash: Vec<u8> = row.get("descriptor_sha256").map_err(sql_error)?;
    if Sha256::digest(config_json.as_bytes()).as_slice() != config_hash
        || descriptor
            .sha256()
            .map_err(|_| error(ErrorCode::WorkflowInvariantViolation))?
            .as_slice()
            != descriptor_hash
        || row.get::<_, usize>("dependency_count").map_err(sql_error)?
            != descriptor.dependencies.len()
    {
        return Err(error(ErrorCode::WorkflowInvariantViolation));
    }
    Ok(descriptor)
}
