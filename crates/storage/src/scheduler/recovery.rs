//! Startup recovery of interrupted scheduler dispatches.

use super::*;

impl SchedulerStore {
    /// Reset interrupted claims before dispatch starts, preserving consumed attempts.
    pub fn reset_claims(&self, now_ms: i64, cron_max_retries: u8) -> Result<(), PlatformError> {
        if cron_max_retries > 3 || now_ms.checked_add(250).is_none() {
            return Err(invalid());
        }
        let mut connection = self.lock()?;
        let tx = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_sql_error)?;
        reset_claims_tx(&tx, now_ms, cron_max_retries)?;
        tx.commit().map_err(map_sql_error)?;
        Ok(())
    }
}

pub(crate) fn reset_claims_tx(
    tx: &rusqlite::Transaction<'_>,
    now_ms: i64,
    cron_max_retries: u8,
) -> Result<(), PlatformError> {
    for sql in include_str!("reset_claims.sql")
        .split(';')
        .filter(|sql| !sql.trim().is_empty())
    {
        let mut statement = tx.prepare(sql).map_err(map_sql_error)?;
        if statement.parameter_count() > 0 {
            statement
                .raw_bind_parameter(1, now_ms)
                .map_err(map_sql_error)?;
        }
        if statement.parameter_count() == 2 {
            statement
                .raw_bind_parameter(2, cron_max_retries)
                .map_err(map_sql_error)?;
        }
        statement.raw_execute().map_err(map_sql_error)?;
    }
    Ok(())
}
