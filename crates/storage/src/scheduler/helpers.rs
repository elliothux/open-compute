//! Scheduler projection validation and stable database errors.

use super::*;

pub(super) fn write_alarm(
    connection: &Connection,
    projection: &AlarmProjection,
    now_ms: i64,
) -> Result<(), PlatformError> {
    validate_projection(projection)?;
    connection
        .execute(
            "INSERT INTO scheduled_jobs
             (id, kind, namespace_resource_id, object_id, object_generation, row_token,
              due_at_ms, target_version_id, execution_generation, state, retry_count,
              last_error_code, created_at_ms, updated_at_ms)
             VALUES (?1, 'do_alarm', ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'scheduled', ?9,
                     NULL, ?10, ?10)
             ON CONFLICT(namespace_resource_id, object_id, object_generation) DO UPDATE SET
               row_token = excluded.row_token,
               due_at_ms = excluded.due_at_ms,
               target_version_id = excluded.target_version_id,
               execution_generation = excluded.execution_generation,
               state = 'scheduled', retry_count = excluded.retry_count,
               last_error_code = NULL,
               updated_at_ms = excluded.updated_at_ms",
            params![
                Uuid::now_v7().hyphenated().to_string(),
                projection.namespace_resource_id.to_string(),
                projection.object_id.to_string(),
                i64::try_from(projection.object_generation).map_err(|_| invalid())?,
                projection.row_token,
                projection.due_at_ms,
                projection.target_version_id.to_string(),
                i64::try_from(projection.execution_generation).map_err(|_| invalid())?,
                i64::from(projection.retry_count),
                now_ms,
            ],
        )
        .map_err(map_sql_error)?;
    Ok(())
}

pub(super) fn validate_projection(projection: &AlarmProjection) -> Result<(), PlatformError> {
    validate_token(&projection.row_token)?;
    if projection.object_generation == 0 || projection.due_at_ms <= 0 || projection.retry_count > 6
    {
        return Err(invalid());
    }
    Ok(())
}

pub(super) fn validate_token(token: &str) -> Result<(), PlatformError> {
    if !(16..=128).contains(&token.len())
        || token
            .bytes()
            .any(|byte| byte.is_ascii_control() || byte.is_ascii_whitespace())
    {
        return Err(invalid());
    }
    Ok(())
}

pub(super) fn map_open_error(error: rusqlite::Error) -> PlatformError {
    map_sql_error(error)
}

pub(super) fn map_sql_error(error: rusqlite::Error) -> PlatformError {
    tracing::warn!(%error, "scheduler SQLite operation failed");
    if let rusqlite::Error::SqliteFailure(code, _) = &error {
        return match code.code {
            rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked => {
                PlatformError::new(ErrorCode::SchedulerBusy, "scheduler database is busy")
            }
            rusqlite::ErrorCode::DatabaseCorrupt | rusqlite::ErrorCode::NotADatabase => corrupt(),
            _ => unavailable(),
        };
    }
    unavailable()
}

pub(super) fn invalid() -> PlatformError {
    PlatformError::new(
        ErrorCode::SchedulerInternalProtocolError,
        "scheduler projection input is invalid",
    )
}

pub(super) fn corrupt() -> PlatformError {
    PlatformError::new(
        ErrorCode::SchedulerCorrupt,
        "scheduler database integrity validation failed",
    )
}

pub(super) fn unavailable() -> PlatformError {
    PlatformError::new(
        ErrorCode::SchedulerUnavailable,
        "scheduler database operation failed",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn busy_classification_uses_sqlite_codes_and_preserves_other_errors() {
        for code in [rusqlite::ffi::SQLITE_BUSY, rusqlite::ffi::SQLITE_LOCKED] {
            let error = rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(code),
                Some("arbitrary localized message".into()),
            );
            assert_eq!(map_sql_error(error).code(), ErrorCode::SchedulerBusy);
        }
        let error = rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_ERROR),
            Some("database is locked".into()),
        );
        assert_eq!(map_sql_error(error).code(), ErrorCode::SchedulerUnavailable);
    }
}
