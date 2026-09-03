//! Per-connection SQLite hardening and scoped tenant execution guards.

use super::engine::{
    D1_MAX_BOUND_PARAMS, D1_MAX_COLUMNS, D1_MAX_SQL_BYTES, D1_MAX_VALUE_OR_ROW_BYTES,
    D1QueryLimits, corrupt_error, limit_error, map_open_error,
};
use open_compute_core::{ErrorCode, PlatformError};
use rusqlite::Connection;
use rusqlite::config::DbConfig;
use rusqlite::hooks::{AuthAction, AuthContext, Authorization};
use rusqlite::limits::Limit;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::time::Instant;

const INTERRUPT_NONE: u8 = 0;
const INTERRUPT_TIMEOUT: u8 = 1;
const INTERRUPT_VM_LIMIT: u8 = 2;
const INTERRUPT_AUTHORIZER: u8 = 3;

/// SQL authority mode installed only around untrusted compilation/execution.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SqlAuthority {
    Tenant,
    Migration,
}

/// Shared reason state used to map SQLite's generic interrupt code.
#[derive(Debug)]
pub(crate) struct ExecutionControl {
    reason: AtomicU8,
    vm_steps: AtomicU64,
    deadline: Instant,
    max_vm_steps: u64,
}

impl ExecutionControl {
    fn new(limits: D1QueryLimits) -> Self {
        Self {
            reason: AtomicU8::new(INTERRUPT_NONE),
            vm_steps: AtomicU64::new(0),
            deadline: Instant::now()
                .checked_add(limits.timeout)
                .unwrap_or_else(Instant::now),
            max_vm_steps: limits.max_vm_steps,
        }
    }

    fn mark(&self, reason: u8) {
        let _ = self.reason.compare_exchange(
            INTERRUPT_NONE,
            reason,
            Ordering::AcqRel,
            Ordering::Acquire,
        );
    }

    fn reason(&self) -> u8 {
        self.reason.load(Ordering::Acquire)
    }
}

/// Apply connection-persistent P0.6 hardening before any tenant statement.
pub(crate) fn configure_connection(
    connection: &Connection,
    quota_bytes: u64,
) -> Result<(), PlatformError> {
    connection
        .execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = FULL;
             PRAGMA foreign_keys = ON;
             PRAGMA trusted_schema = OFF;
             PRAGMA recursive_triggers = ON;
             PRAGMA temp_store = MEMORY;
             PRAGMA busy_timeout = 5000;
             PRAGMA wal_autocheckpoint = 1000;
             PRAGMA mmap_size = 0;",
        )
        .map_err(|error| map_open_error(&error))?;
    connection
        .set_db_config(DbConfig::SQLITE_DBCONFIG_DEFENSIVE, true)
        .map_err(|_| unavailable())?;
    connection
        .set_db_config(DbConfig::SQLITE_DBCONFIG_TRUSTED_SCHEMA, false)
        .map_err(|_| unavailable())?;
    connection
        .set_db_config(DbConfig::SQLITE_DBCONFIG_DQS_DDL, false)
        .map_err(|_| unavailable())?;
    connection
        .set_db_config(DbConfig::SQLITE_DBCONFIG_DQS_DML, false)
        .map_err(|_| unavailable())?;
    set_runtime_limits(connection)?;
    let page_size: i64 = connection
        .query_row("PRAGMA page_size", [], |row| row.get(0))
        .map_err(|error| map_open_error(&error))?;
    let page_size = u64::try_from(page_size).map_err(|_| unavailable())?;
    if page_size == 0 {
        return Err(unavailable());
    }
    let pages = quota_bytes
        .checked_add(page_size - 1)
        .and_then(|value| value.checked_div(page_size))
        .ok_or_else(limit_error)?;
    let pages = i64::try_from(pages).map_err(|_| limit_error())?;
    connection
        .pragma_update(None, "max_page_count", pages)
        .map_err(|error| map_open_error(&error))?;
    Ok(())
}

fn set_runtime_limits(connection: &Connection) -> Result<(), PlatformError> {
    for (limit, value) in [
        (Limit::SQLITE_LIMIT_LENGTH, D1_MAX_VALUE_OR_ROW_BYTES),
        (Limit::SQLITE_LIMIT_SQL_LENGTH, D1_MAX_SQL_BYTES),
        (Limit::SQLITE_LIMIT_COLUMN, D1_MAX_COLUMNS),
        (Limit::SQLITE_LIMIT_VARIABLE_NUMBER, D1_MAX_BOUND_PARAMS),
        (Limit::SQLITE_LIMIT_FUNCTION_ARG, 32),
        (Limit::SQLITE_LIMIT_LIKE_PATTERN_LENGTH, 50),
        (Limit::SQLITE_LIMIT_ATTACHED, 0),
        (Limit::SQLITE_LIMIT_COMPOUND_SELECT, 100),
        (Limit::SQLITE_LIMIT_TRIGGER_DEPTH, 32),
        (Limit::SQLITE_LIMIT_EXPR_DEPTH, 1000),
        (Limit::SQLITE_LIMIT_WORKER_THREADS, 0),
    ] {
        connection
            .set_limit(limit, i32::try_from(value).map_err(|_| limit_error())?)
            .map_err(|_| unavailable())?;
    }
    Ok(())
}

/// Install authorizer and progress callback for one scoped operation.
pub(crate) fn install_guard(
    connection: &Connection,
    limits: D1QueryLimits,
    authority: SqlAuthority,
) -> Arc<ExecutionControl> {
    let control = Arc::new(ExecutionControl::new(limits));
    reinstall_guard(connection, authority, control.clone());
    control
}

/// Reinstall transient callbacks without resetting one operation's deadline or VM budget.
pub(crate) fn reinstall_guard(
    connection: &Connection,
    authority: SqlAuthority,
    control: Arc<ExecutionControl>,
) {
    let auth_control = control.clone();
    connection.authorizer(Some(move |context: AuthContext<'_>| {
        let decision = authorize(context, authority);
        if decision == Authorization::Deny {
            auth_control.mark(INTERRUPT_AUTHORIZER);
        }
        decision
    }));
    let progress_control = control.clone();
    connection.progress_handler(
        1000,
        Some(move || {
            if Instant::now() >= progress_control.deadline {
                progress_control.mark(INTERRUPT_TIMEOUT);
                return true;
            }
            let steps = progress_control
                .vm_steps
                .fetch_add(1000, Ordering::AcqRel)
                .saturating_add(1000);
            if steps > progress_control.max_vm_steps {
                progress_control.mark(INTERRUPT_VM_LIMIT);
                return true;
            }
            false
        }),
    );
}

/// Remove every transient callback before host transaction control or reuse.
pub(crate) fn remove_guard(connection: &Connection) {
    connection.authorizer(None::<fn(AuthContext<'_>) -> Authorization>);
    connection.progress_handler(0, None::<fn() -> bool>);
}

/// Map a secret-bearing SQLite error into a stable D1 category.
pub(crate) fn map_sqlite_error(
    error: &rusqlite::Error,
    control: &ExecutionControl,
) -> PlatformError {
    use rusqlite::ffi::ErrorCode as SqliteCode;
    if is_sqlite_limit_failure(error) {
        return limit_error();
    }
    match error.sqlite_error_code() {
        Some(SqliteCode::OperationInterrupted) if control.reason() == INTERRUPT_TIMEOUT => {
            PlatformError::new(
                ErrorCode::D1Timeout,
                "D1 operation exceeded its wall deadline",
            )
        }
        Some(SqliteCode::OperationInterrupted) => limit_error(),
        Some(SqliteCode::AuthorizationForStatementDenied) => PlatformError::new(
            ErrorCode::D1AuthorizerDenied,
            "D1 SQL was denied by the SQLite authorizer",
        ),
        Some(SqliteCode::TooBig | SqliteCode::OutOfMemory) => limit_error(),
        Some(SqliteCode::ParameterOutOfRange | SqliteCode::TypeMismatch) => PlatformError::new(
            ErrorCode::D1ParameterMismatch,
            "D1 bound parameter count or type does not match",
        ),
        Some(SqliteCode::DiskFull) => PlatformError::new(
            ErrorCode::D1DatabaseFull,
            "D1 database quota or disk capacity was reached",
        ),
        Some(SqliteCode::DatabaseBusy | SqliteCode::DatabaseLocked) => {
            PlatformError::new(ErrorCode::D1Overloaded, "D1 database is temporarily busy")
        }
        Some(SqliteCode::DatabaseCorrupt | SqliteCode::NotADatabase) => corrupt_error(),
        Some(SqliteCode::SystemIoFailure | SqliteCode::CannotOpen) => {
            PlatformError::new(ErrorCode::ResourceUnavailable, "D1 database is unavailable")
        }
        _ if control.reason() == INTERRUPT_AUTHORIZER => PlatformError::new(
            ErrorCode::D1AuthorizerDenied,
            "D1 SQL was denied by the SQLite authorizer",
        ),
        _ => PlatformError::new(ErrorCode::D1SqlInvalid, "D1 SQL could not be executed"),
    }
}

fn is_sqlite_limit_failure(error: &rusqlite::Error) -> bool {
    let (rusqlite::Error::SqliteFailure(_, Some(message))
    | rusqlite::Error::SqlInputError { msg: message, .. }) = error
    else {
        return false;
    };
    [
        "too many columns",
        "too many arguments on function",
        "LIKE or GLOB pattern too complex",
        "too many terms in compound SELECT",
        "Expression tree is too large",
        "too many levels of trigger recursion",
        "too many SQL variables",
    ]
    .iter()
    .any(|prefix| message.starts_with(prefix))
}

fn authorize(context: AuthContext<'_>, authority: SqlAuthority) -> Authorization {
    use AuthAction::{
        AlterTable, Attach, CreateTempIndex, CreateTempTable, CreateTempTrigger, CreateTempView,
        CreateVtable, Delete, Detach, DropTempIndex, DropTempTable, DropTempTrigger, DropTempView,
        DropVtable, Function, Insert, Pragma, Read, Savepoint, Transaction, Unknown, Update,
    };
    if context
        .database_name
        .is_some_and(|database| database != "main")
    {
        return Authorization::Deny;
    }
    let internal = match context.action {
        Read { table_name, .. }
        | Insert { table_name }
        | Delete { table_name }
        | Update { table_name, .. } => table_name.starts_with("__open_compute_"),
        _ => false,
    };
    if internal {
        return Authorization::Deny;
    }
    match context.action {
        Attach { .. }
        | Detach { .. }
        | Transaction { .. }
        | Savepoint { .. }
        | CreateTempIndex { .. }
        | CreateTempTable { .. }
        | CreateTempTrigger { .. }
        | CreateTempView { .. }
        | DropTempIndex { .. }
        | DropTempTable { .. }
        | DropTempTrigger { .. }
        | DropTempView { .. }
        | Unknown { .. } => Authorization::Deny,
        AlterTable { database_name, .. } if database_name != "main" => Authorization::Deny,
        CreateVtable { module_name, .. } | DropVtable { module_name, .. }
            if !module_name.eq_ignore_ascii_case("fts5") =>
        {
            Authorization::Deny
        }
        Function { function_name } if function_name.eq_ignore_ascii_case("load_extension") => {
            Authorization::Deny
        }
        Pragma {
            pragma_name,
            pragma_value,
        } => authorize_pragma(pragma_name, pragma_value, authority),
        _ => Authorization::Allow,
    }
}

fn authorize_pragma(name: &str, value: Option<&str>, authority: SqlAuthority) -> Authorization {
    let name = name.to_ascii_lowercase();
    let introspection = [
        "table_info",
        "table_xinfo",
        "index_list",
        "index_info",
        "index_xinfo",
        "foreign_key_list",
        "foreign_key_check",
        "database_list",
    ];
    if introspection.contains(&name.as_str()) {
        return Authorization::Allow;
    }
    if value.is_none() && matches!(name.as_str(), "user_version" | "application_id") {
        return Authorization::Allow;
    }
    if authority == SqlAuthority::Migration
        && value.is_some()
        && matches!(name.as_str(), "user_version" | "application_id")
    {
        return Authorization::Allow;
    }
    Authorization::Deny
}

fn unavailable() -> PlatformError {
    PlatformError::new(
        ErrorCode::ResourceUnavailable,
        "D1 database hardening failed",
    )
}
