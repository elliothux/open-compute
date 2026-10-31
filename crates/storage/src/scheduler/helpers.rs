//! Scheduler projection validation and stable database errors.

use super::*;

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
