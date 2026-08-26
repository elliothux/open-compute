//! Queue catalog validation, audit, and stable error helpers.

use open_compute_core::{AccountId, ErrorCode, PlatformError, QueueId, RequestId};
use rusqlite::{Transaction, params};

pub(super) fn audit(
    tx: &Transaction<'_>,
    account_id: AccountId,
    action: &str,
    queue_id: QueueId,
    request_id: RequestId,
    now_ms: i64,
) -> Result<(), PlatformError> {
    tx.execute(
        "INSERT INTO control_audit_events
         (account_id, action, target_type, target_id, request_id, details_json, created_at_ms)
         VALUES (?1, ?2, 'queue', ?3, ?4, X'7B7D', ?5)",
        params![
            account_id.to_string(),
            action,
            queue_id.to_string(),
            request_id.to_string(),
            now_ms,
        ],
    )
    .map_err(|_| db_error())?;
    Ok(())
}

pub(super) fn validate_name(name: &str) -> Result<(), PlatformError> {
    if name.is_empty() || name.len() > 128 || name.chars().any(char::is_control) {
        return Err(PlatformError::new(
            ErrorCode::ConfigInvalid,
            "Queue display name is invalid",
        ));
    }
    Ok(())
}

pub(super) fn collect<T>(
    rows: rusqlite::MappedRows<'_, impl FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<T>>,
) -> Result<Vec<T>, PlatformError> {
    let mut output = Vec::new();
    for row in rows {
        output.push(row.map_err(|_| invariant())?);
    }
    Ok(output)
}

pub(super) fn not_found() -> PlatformError {
    PlatformError::new(ErrorCode::QueueNotFound, "Queue was not found")
}

pub(super) fn not_ready() -> PlatformError {
    PlatformError::new(
        ErrorCode::QueueNotReady,
        "Queue lifecycle does not admit this operation",
    )
}

pub(super) fn invariant() -> PlatformError {
    PlatformError::new(
        ErrorCode::QueueInvariantViolation,
        "Queue authority invariant failed",
    )
}

pub(super) fn db_error() -> PlatformError {
    PlatformError::new(
        ErrorCode::Internal,
        "Queue control database operation failed",
    )
}
