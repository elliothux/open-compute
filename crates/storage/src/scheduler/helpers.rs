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

pub(super) fn random_token() -> Result<String, PlatformError> {
    let mut bytes = [0_u8; 32];
    rand::rngs::OsRng
        .try_fill_bytes(&mut bytes)
        .map_err(|_| unavailable())?;
    Ok(hex::encode(bytes))
}

pub(super) fn map_open_error(error: rusqlite::Error) -> PlatformError {
    map_sql_error(error)
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "the callback contract transfers ownership of this value"
)]
pub(super) fn map_sql_error(error: rusqlite::Error) -> PlatformError {
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
