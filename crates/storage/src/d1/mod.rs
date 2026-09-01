//! Product-owned D1 catalog, secure paths, and hardened SQLite engine.

mod backup;
mod catalog;
mod engine;
mod execution;
mod hardening;
mod paths;

pub use catalog::{D1BackupRecord, D1BackupState, D1DatabaseRecord, D1DatabaseRepository};
pub use engine::{
    D1_DATABASE_SCHEMA_VERSION, D1_MAX_BATCH_STATEMENTS, D1_MAX_BOUND_PARAMS, D1_MAX_COLUMNS,
    D1_MAX_EXEC_STATEMENTS, D1_MAX_SQL_BYTES, D1_MAX_VALUE_OR_ROW_BYTES, D1Engine, D1ExecResult,
    D1Meta, D1Migration, D1MigrationRecord, D1QueryLimits, D1QueryTimings, D1Statement,
    D1StatementResult, D1Value,
};
pub use paths::D1Paths;

#[cfg(test)]
mod tests;
