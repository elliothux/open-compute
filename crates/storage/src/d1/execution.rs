//! Prepared statement, transactional batch, tail-parser exec, and migrations.

use super::engine::{
    D1_MAX_BATCH_STATEMENTS, D1_MAX_BOUND_PARAMS, D1_MAX_COLUMNS, D1_MAX_EXEC_STATEMENTS,
    D1_MAX_SQL_BYTES, D1_MAX_VALUE_OR_ROW_BYTES, D1Engine, D1ExecResult, D1Meta, D1Migration,
    D1MigrationRecord, D1QueryLimits, D1Statement, D1StatementResult, D1Value,
    bump_session_version, limit_error,
};
use super::hardening::{
    ExecutionControl, SqlAuthority, install_guard, map_sqlite_error, reinstall_guard, remove_guard,
};
use fallible_iterator::FallibleIterator as _;
use open_compute_core::{ErrorCode, PlatformError};
use rusqlite::types::{Null, ValueRef};
use rusqlite::{Batch, Connection, Statement, params};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Instant;

const D1_MAX_MIGRATIONS: usize = 100;

impl D1Engine {
    /// Compile statements under the tenant authorizer and classify read-only
    /// status before permission admission. No statement is stepped.
    pub fn statements_readonly(
        &self,
        statements: &[D1Statement],
        limits: D1QueryLimits,
    ) -> Result<bool, PlatformError> {
        if statements.is_empty() || statements.len() > D1_MAX_BATCH_STATEMENTS {
            return Err(invalid_batch());
        }
        for statement in statements {
            validate_statement(statement)?;
        }
        let connection = self.open()?;
        let control = install_guard(&connection, limits, SqlAuthority::Tenant);
        let result = (|| {
            let mut readonly = true;
            for statement in statements {
                let prepared = connection
                    .prepare(&statement.sql)
                    .map_err(|error| map_sqlite_error(&error, &control))?;
                if prepared.parameter_count() != statement.params.len() {
                    return Err(parameter_mismatch());
                }
                readonly &= prepared.readonly();
            }
            Ok(readonly)
        })();
        remove_guard(&connection);
        result
    }

    /// Execute one prepared statement through the tenant authorizer.
    pub fn query(
        &self,
        statement: &D1Statement,
        limits: D1QueryLimits,
    ) -> Result<D1StatementResult, PlatformError> {
        validate_statement(statement)?;
        let connection = self.open()?;
        let preflight = install_guard(&connection, limits, SqlAuthority::Tenant);
        let readonly = (|| {
            let prepared = connection
                .prepare(&statement.sql)
                .map_err(|error| map_sqlite_error(&error, &preflight))?;
            if prepared.parameter_count() != statement.params.len() {
                return Err(parameter_mismatch());
            }
            Ok(prepared.readonly())
        })();
        remove_guard(&connection);
        let readonly = readonly?;
        if !readonly {
            connection
                .execute_batch("BEGIN IMMEDIATE")
                .map_err(|error| map_sqlite_error(&error, &preflight))?;
        }
        let control = install_guard(&connection, limits, SqlAuthority::Tenant);
        let result = materialize(&connection, statement, limits, &control);
        remove_guard(&connection);
        let mut result = match result {
            Ok(result) => result,
            Err(error) => {
                if !readonly {
                    let _ = connection.execute_batch("ROLLBACK");
                }
                return Err(error);
            }
        };
        if !readonly {
            if let Err(error) = bump_session_version(&connection) {
                let _ = connection.execute_batch("ROLLBACK");
                return Err(error);
            }
            if connection.execute_batch("COMMIT").is_err() {
                let _ = connection.execute_batch("ROLLBACK");
                return Err(PlatformError::new(
                    ErrorCode::D1ResultUnknown,
                    "D1 statement commit result is unknown",
                ));
            }
        }
        result.meta.size_after = logical_size(&connection)?;
        Ok(result)
    }

    /// Execute a non-empty wire-bounded statement list in one SQLite transaction.
    pub fn batch(
        &self,
        statements: &[D1Statement],
        limits: D1QueryLimits,
    ) -> Result<Vec<D1StatementResult>, PlatformError> {
        if statements.is_empty() || statements.len() > D1_MAX_BATCH_STATEMENTS {
            return Err(invalid_batch());
        }
        for statement in statements {
            validate_statement(statement).map_err(|_| invalid_batch())?;
        }
        let connection = self.open()?;
        let preflight = install_guard(&connection, limits, SqlAuthority::Tenant);
        let mut any_write = false;
        let mut preflight_error = None;
        for statement in statements {
            match connection.prepare(&statement.sql) {
                Ok(prepared) => {
                    if prepared.parameter_count() != statement.params.len() {
                        preflight_error = Some(parameter_mismatch());
                        break;
                    }
                    any_write |= !prepared.readonly();
                }
                Err(error) => {
                    preflight_error = Some(map_sqlite_error(&error, &preflight));
                    break;
                }
            }
        }
        remove_guard(&connection);
        if let Some(error) = preflight_error {
            return Err(error);
        }

        connection
            .execute_batch(if any_write {
                "BEGIN IMMEDIATE"
            } else {
                "BEGIN"
            })
            .map_err(|error| map_sqlite_error(&error, &preflight))?;
        let control = install_guard(&connection, limits, SqlAuthority::Tenant);
        let mut results = Vec::with_capacity(statements.len());
        let mut total_rows = 0_usize;
        let mut total_bytes = 0_usize;
        let execution = (|| {
            for statement in statements {
                let result = materialize(&connection, statement, limits, &control)?;
                total_rows = total_rows
                    .checked_add(result.rows.len())
                    .ok_or_else(limit_error)?;
                total_bytes = total_bytes
                    .checked_add(result_size(&result))
                    .ok_or_else(limit_error)?;
                if total_rows > limits.max_result_rows || total_bytes > limits.max_result_bytes {
                    return Err(limit_error());
                }
                results.push(result);
            }
            Ok(())
        })();
        remove_guard(&connection);
        if let Err(error) = execution {
            let _ = connection.execute_batch("ROLLBACK");
            return Err(error);
        }
        if any_write && let Err(error) = bump_session_version(&connection) {
            let _ = connection.execute_batch("ROLLBACK");
            return Err(error);
        }
        if connection.execute_batch("COMMIT").is_err() {
            let _ = connection.execute_batch("ROLLBACK");
            return Err(PlatformError::new(
                ErrorCode::D1ResultUnknown,
                "D1 batch commit result is unknown",
            ));
        }
        let size = logical_size(&connection)?;
        for result in &mut results {
            result.meta.size_after = size;
        }
        Ok(results)
    }

    /// Execute query objects, including semicolon-delimited SQL, in one transaction.
    pub fn query_batch(
        &self,
        queries: &[D1Statement],
        limits: D1QueryLimits,
    ) -> Result<Vec<D1StatementResult>, PlatformError> {
        if queries.is_empty() || queries.len() > D1_MAX_BATCH_STATEMENTS {
            return Err(invalid_batch());
        }
        for query in queries {
            validate_statement(query)?;
        }
        let connection = self.open()?;
        let transaction_control = install_guard(&connection, limits, SqlAuthority::Tenant);
        remove_guard(&connection);
        connection
            .execute_batch("BEGIN IMMEDIATE")
            .map_err(|error| map_sqlite_error(&error, &transaction_control))?;
        let control = install_guard(&connection, limits, SqlAuthority::Tenant);
        let mut results = Vec::with_capacity(queries.len());
        let mut statement_count = 0_usize;
        let mut any_write = false;
        let mut total_rows = 0_usize;
        let mut total_bytes = 0_usize;
        let execution = (|| {
            for query in queries {
                let mut params_used = 0_usize;
                let mut batch = Batch::new(&connection, &query.sql);
                while let Some(statement) = batch
                    .next()
                    .map_err(|error| map_sqlite_error(&error, &control))?
                {
                    statement_count = statement_count.checked_add(1).ok_or_else(limit_error)?;
                    if statement_count > D1_MAX_BATCH_STATEMENTS {
                        return Err(limit_error());
                    }
                    let next = params_used
                        .checked_add(statement.parameter_count())
                        .ok_or_else(limit_error)?;
                    if next > query.params.len() {
                        return Err(parameter_mismatch());
                    }
                    any_write |= !statement.readonly();
                    let result = materialize_prepared(
                        &connection,
                        statement,
                        &query.params[params_used..next],
                        limits,
                        &control,
                    )?;
                    params_used = next;
                    total_rows = total_rows
                        .checked_add(result.rows.len())
                        .ok_or_else(limit_error)?;
                    total_bytes = total_bytes
                        .checked_add(result_size(&result))
                        .ok_or_else(limit_error)?;
                    if total_rows > limits.max_result_rows || total_bytes > limits.max_result_bytes
                    {
                        return Err(limit_error());
                    }
                    results.push(result);
                }
                if params_used != query.params.len() {
                    return Err(parameter_mismatch());
                }
            }
            if statement_count == 0 {
                return Err(sql_invalid());
            }
            Ok(())
        })();
        remove_guard(&connection);
        if let Err(error) = execution {
            let _ = connection.execute_batch("ROLLBACK");
            return Err(error);
        }
        if any_write && let Err(error) = bump_session_version(&connection) {
            let _ = connection.execute_batch("ROLLBACK");
            return Err(error);
        }
        if connection.execute_batch("COMMIT").is_err() {
            let _ = connection.execute_batch("ROLLBACK");
            return Err(PlatformError::new(
                ErrorCode::D1ResultUnknown,
                "D1 query batch commit result is unknown",
            ));
        }
        let size = logical_size(&connection)?;
        for result in &mut results {
            result.meta.size_after = size;
        }
        Ok(results)
    }

    /// Execute all statements in one bounded SQL input using SQLite's parser tail pointer.
    pub fn exec(&self, sql: &str, limits: D1QueryLimits) -> Result<D1ExecResult, PlatformError> {
        validate_sql(sql)?;
        let connection = self.open()?;
        let started = Instant::now();
        let control = install_guard(&connection, limits, SqlAuthority::Tenant);
        let result =
            execute_tail_batch_versioned(&connection, sql, D1_MAX_EXEC_STATEMENTS, &control);
        remove_guard(&connection);
        let count = result?;
        if count == 0 {
            return Err(sql_invalid());
        }
        Ok(D1ExecResult {
            count: u32::try_from(count).map_err(|_| limit_error())?,
            duration: started.elapsed().as_secs_f64() * 1000.0,
        })
    }

    /// List the private migration ledger without exposing SQL text.
    pub fn migrations(&self) -> Result<Vec<D1MigrationRecord>, PlatformError> {
        let connection = self.open()?;
        let mut statement = connection
            .prepare(
                "SELECT id, name, sha256, applied_at_ms
             FROM __open_compute_migrations ORDER BY id",
            )
            .map_err(|_| migration_internal())?;
        let rows = statement
            .query_map([], |row| {
                let id: i64 = row.get(0)?;
                let digest: Vec<u8> = row.get(2)?;
                Ok(D1MigrationRecord {
                    id: u32::try_from(id).map_err(|_| rusqlite::Error::InvalidQuery)?,
                    name: row.get(1)?,
                    sha256: hex::encode(digest),
                    applied_at_ms: row.get(3)?,
                })
            })
            .map_err(|_| migration_internal())?;
        rows.map(|row| row.map_err(|_| migration_internal()))
            .collect()
    }

    /// Apply all pending ordered migrations in one transaction and history version.
    pub fn apply_migrations(
        &self,
        migrations: &[D1Migration],
        limits: D1QueryLimits,
        now_ms: i64,
    ) -> Result<Vec<D1MigrationRecord>, PlatformError> {
        if migrations.is_empty() || migrations.len() > D1_MAX_MIGRATIONS {
            return Err(sql_invalid());
        }
        let mut ids = BTreeSet::new();
        let mut names = BTreeSet::new();
        for migration in migrations {
            if migration.id == 0
                || migration.name.is_empty()
                || migration.name.len() > 255
                || !ids.insert(migration.id)
                || !names.insert(migration.name.as_str())
                || Sha256::digest(migration.sql.as_bytes()).as_slice() != migration.sha256
            {
                return Err(migration_drift());
            }
            validate_sql(&migration.sql)?;
        }

        let connection = self.open()?;
        let mut applied = read_migration_map(&connection)?;
        let mut pending = Vec::new();
        for migration in migrations {
            if let Some(existing) = applied.get(&migration.id) {
                if existing.name == migration.name
                    && existing.sha256 == hex::encode(migration.sha256)
                {
                    continue;
                }
                return Err(migration_drift());
            }
            if applied
                .values()
                .any(|existing| existing.name == migration.name)
            {
                return Err(migration_drift());
            }
            let expected = applied
                .keys()
                .next_back()
                .copied()
                .unwrap_or(0)
                .saturating_add(1);
            if migration.id != expected {
                return Err(migration_drift());
            }
            pending.push(migration);
            applied.insert(
                migration.id,
                D1MigrationRecord {
                    id: migration.id,
                    name: migration.name.clone(),
                    sha256: hex::encode(migration.sha256),
                    applied_at_ms: now_ms,
                },
            );
        }
        if pending.is_empty() {
            return Ok(applied.into_values().collect());
        }
        connection
            .execute_batch("BEGIN IMMEDIATE")
            .map_err(|_| migration_internal())?;
        for migration in pending {
            let control = install_guard(&connection, limits, SqlAuthority::Migration);
            let execution = execute_tail_batch(
                &connection,
                &migration.sql,
                D1_MAX_EXEC_STATEMENTS,
                &control,
            );
            remove_guard(&connection);
            match execution {
                Ok((0, _, _)) => {
                    let _ = connection.execute_batch("ROLLBACK");
                    return Err(sql_invalid());
                }
                Ok(_) => {}
                Err(error) => {
                    let _ = connection.execute_batch("ROLLBACK");
                    return Err(error);
                }
            }
            let insert = connection.execute(
                "INSERT INTO __open_compute_migrations(id, name, sha256, applied_at_ms)
                 VALUES (?1, ?2, ?3, ?4)",
                params![
                    i64::from(migration.id),
                    migration.name,
                    migration.sha256.as_slice(),
                    now_ms
                ],
            );
            if insert.is_err() {
                let _ = connection.execute_batch("ROLLBACK");
                return Err(migration_internal());
            }
        }
        if let Err(error) = bump_session_version(&connection) {
            let _ = connection.execute_batch("ROLLBACK");
            return Err(error);
        }
        if connection.execute_batch("COMMIT").is_err() {
            let _ = connection.execute_batch("ROLLBACK");
            return Err(PlatformError::new(
                ErrorCode::D1ResultUnknown,
                "D1 migration commit result is unknown",
            ));
        }
        Ok(applied.into_values().collect())
    }
}

fn materialize(
    connection: &Connection,
    input: &D1Statement,
    limits: D1QueryLimits,
    control: &Arc<ExecutionControl>,
) -> Result<D1StatementResult, PlatformError> {
    let statement = connection
        .prepare(&input.sql)
        .map_err(|error| map_sqlite_error(&error, control))?;
    materialize_prepared(connection, statement, &input.params, limits, control)
}

fn materialize_prepared(
    connection: &Connection,
    mut statement: Statement<'_>,
    params: &[D1Value],
    limits: D1QueryLimits,
    control: &Arc<ExecutionControl>,
) -> Result<D1StatementResult, PlatformError> {
    let started = Instant::now();
    if statement.parameter_count() != params.len() {
        return Err(parameter_mismatch());
    }
    if statement.column_count() > D1_MAX_COLUMNS {
        return Err(limit_error());
    }
    bind(&mut statement, params, control)?;
    let readonly = statement.readonly();
    let columns = statement
        .column_names()
        .into_iter()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let mut rows = statement.raw_query();
    let mut output = Vec::new();
    let mut result_bytes = columns.iter().map(String::len).sum::<usize>();
    while let Some(row) = rows
        .next()
        .map_err(|error| map_sqlite_error(&error, control))?
    {
        if output.len() >= limits.max_result_rows {
            return Err(limit_error());
        }
        let mut values = Vec::with_capacity(columns.len());
        let mut row_bytes = 0_usize;
        for index in 0..columns.len() {
            let value = value_from_ref(
                row.get_ref(index)
                    .map_err(|error| map_sqlite_error(&error, control))?,
            )?;
            row_bytes = row_bytes
                .checked_add(value.byte_len())
                .ok_or_else(limit_error)?;
            values.push(value);
        }
        if row_bytes > D1_MAX_VALUE_OR_ROW_BYTES {
            return Err(limit_error());
        }
        result_bytes = result_bytes
            .checked_add(row_bytes)
            .ok_or_else(limit_error)?;
        if result_bytes > limits.max_result_bytes {
            return Err(limit_error());
        }
        output.push(values);
    }
    drop(rows);
    drop(statement);
    let changes = if readonly { 0 } else { connection.changes() };
    let duration = started.elapsed().as_secs_f64() * 1000.0;
    Ok(D1StatementResult {
        columns,
        meta: D1Meta::local(
            duration,
            changes,
            connection.last_insert_rowid(),
            !readonly,
            0,
            u64::try_from(output.len()).map_err(|_| limit_error())?,
            changes,
        ),
        rows: output,
    })
}

fn bind(
    statement: &mut Statement<'_>,
    values: &[D1Value],
    control: &ExecutionControl,
) -> Result<(), PlatformError> {
    for (offset, value) in values.iter().enumerate() {
        let index = offset + 1;
        let result = match value {
            D1Value::Null => statement.raw_bind_parameter(index, Null),
            D1Value::Integer(value) => statement.raw_bind_parameter(index, value),
            D1Value::Real(value) if value.is_finite() => statement.raw_bind_parameter(index, value),
            D1Value::Real(_) => {
                return Err(PlatformError::new(
                    ErrorCode::D1TypeError,
                    "D1 numbers must be finite",
                ));
            }
            D1Value::Text(value) => statement.raw_bind_parameter(index, value.as_str()),
            D1Value::Blob(value) => statement.raw_bind_parameter(index, value.as_slice()),
        };
        result.map_err(|error| map_sqlite_error(&error, control))?;
    }
    Ok(())
}

fn value_from_ref(value: ValueRef<'_>) -> Result<D1Value, PlatformError> {
    match value {
        ValueRef::Null => Ok(D1Value::Null),
        ValueRef::Integer(value) => Ok(D1Value::Integer(value)),
        ValueRef::Real(value) if value.is_finite() => Ok(D1Value::Real(value)),
        ValueRef::Real(_) => Err(PlatformError::new(
            ErrorCode::D1InternalProtocolError,
            "SQLite returned a non-finite D1 value",
        )),
        ValueRef::Text(value) => std::str::from_utf8(value)
            .map(|value| D1Value::Text(value.to_owned()))
            .map_err(|_| {
                PlatformError::new(
                    ErrorCode::D1InternalProtocolError,
                    "SQLite returned invalid UTF-8 text",
                )
            }),
        ValueRef::Blob(value) => Ok(D1Value::Blob(value.to_vec())),
    }
}

pub(super) fn execute_tail_batch(
    connection: &Connection,
    sql: &str,
    maximum: usize,
    control: &ExecutionControl,
) -> Result<(usize, bool, u64), PlatformError> {
    let mut batch = Batch::new(connection, sql);
    let mut count = 0_usize;
    let mut any_write = false;
    let mut rows_read = 0_u64;
    while let Some(mut statement) = batch
        .next()
        .map_err(|error| map_sqlite_error(&error, control))?
    {
        count = count.checked_add(1).ok_or_else(limit_error)?;
        if count > maximum {
            return Err(limit_error());
        }
        if statement.parameter_count() != 0 {
            return Err(parameter_mismatch());
        }
        let write = !statement.readonly();
        any_write |= write;
        let mut rows = statement.raw_query();
        while rows
            .next()
            .map_err(|error| map_sqlite_error(&error, control))?
            .is_some()
        {
            rows_read = rows_read.checked_add(1).ok_or_else(limit_error)?;
        }
    }
    Ok((count, any_write, rows_read))
}

fn execute_tail_batch_versioned(
    connection: &Connection,
    sql: &str,
    maximum: usize,
    control: &Arc<ExecutionControl>,
) -> Result<usize, PlatformError> {
    let mut batch = Batch::new(connection, sql);
    let mut count = 0_usize;
    let mut version_persisted = false;
    while let Some(mut statement) = batch
        .next()
        .map_err(|error| map_sqlite_error(&error, control))?
    {
        count = count.checked_add(1).ok_or_else(limit_error)?;
        if count > maximum {
            return Err(limit_error());
        }
        if statement.parameter_count() != 0 {
            return Err(parameter_mismatch());
        }
        if !statement.readonly() && !version_persisted {
            remove_guard(connection);
            if let Err(error) = connection.execute_batch("BEGIN IMMEDIATE") {
                return Err(map_sqlite_error(&error, control));
            }
            if let Err(error) = bump_session_version(connection) {
                let _ = connection.execute_batch("ROLLBACK");
                return Err(error);
            }
            if connection.execute_batch("COMMIT").is_err() {
                let _ = connection.execute_batch("ROLLBACK");
                return Err(result_unknown());
            }
            version_persisted = true;
            reinstall_guard(connection, SqlAuthority::Tenant, control.clone());
        }
        let mut rows = statement.raw_query();
        while rows
            .next()
            .map_err(|error| map_sqlite_error(&error, control))?
            .is_some()
        {}
    }
    Ok(count)
}

fn result_unknown() -> PlatformError {
    PlatformError::new(
        ErrorCode::D1ResultUnknown,
        "D1 exec history version durability is unknown",
    )
}

pub(super) fn logical_size(connection: &Connection) -> Result<u64, PlatformError> {
    let pages: i64 = connection
        .query_row("PRAGMA page_count", [], |row| row.get(0))
        .map_err(|_| migration_internal())?;
    let page_size: i64 = connection
        .query_row("PRAGMA page_size", [], |row| row.get(0))
        .map_err(|_| migration_internal())?;
    u64::try_from(pages)
        .ok()
        .and_then(|pages| {
            u64::try_from(page_size)
                .ok()
                .and_then(|size| pages.checked_mul(size))
        })
        .ok_or_else(limit_error)
}

fn read_migration_map(
    connection: &Connection,
) -> Result<BTreeMap<u32, D1MigrationRecord>, PlatformError> {
    let mut statement = connection
        .prepare(
            "SELECT id, name, sha256, applied_at_ms FROM __open_compute_migrations ORDER BY id",
        )
        .map_err(|_| migration_internal())?;
    let rows = statement
        .query_map([], |row| {
            let id: i64 = row.get(0)?;
            let digest: Vec<u8> = row.get(2)?;
            let id = u32::try_from(id).map_err(|_| rusqlite::Error::InvalidQuery)?;
            Ok((
                id,
                D1MigrationRecord {
                    id,
                    name: row.get(1)?,
                    sha256: hex::encode(digest),
                    applied_at_ms: row.get(3)?,
                },
            ))
        })
        .map_err(|_| migration_internal())?;
    rows.map(|row| row.map_err(|_| migration_internal()))
        .collect()
}

fn validate_statement(statement: &D1Statement) -> Result<(), PlatformError> {
    validate_sql(&statement.sql)?;
    if statement.params.len() > D1_MAX_BOUND_PARAMS
        || statement.params.iter().any(|value| {
            value.byte_len() > D1_MAX_VALUE_OR_ROW_BYTES
                || matches!(value, D1Value::Real(number) if !number.is_finite())
        })
    {
        return Err(limit_error());
    }
    Ok(())
}

fn validate_sql(sql: &str) -> Result<(), PlatformError> {
    if sql.trim().is_empty() || sql.len() > D1_MAX_SQL_BYTES {
        return Err(sql_invalid());
    }
    Ok(())
}

fn result_size(result: &D1StatementResult) -> usize {
    result.columns.iter().map(String::len).sum::<usize>()
        + result
            .rows
            .iter()
            .flatten()
            .map(D1Value::byte_len)
            .sum::<usize>()
}

fn sql_invalid() -> PlatformError {
    PlatformError::new(
        ErrorCode::D1SqlInvalid,
        "D1 SQL must contain one or more bounded statements",
    )
}

fn parameter_mismatch() -> PlatformError {
    PlatformError::new(
        ErrorCode::D1ParameterMismatch,
        "D1 bound parameter count does not match the statement",
    )
}

fn invalid_batch() -> PlatformError {
    PlatformError::new(
        ErrorCode::D1InvalidBatch,
        "D1 batch must contain statements from one owner",
    )
}

fn migration_drift() -> PlatformError {
    PlatformError::new(
        ErrorCode::D1MigrationDrift,
        "D1 migration identity or ordering drifted",
    )
}

fn migration_internal() -> PlatformError {
    PlatformError::new(
        ErrorCode::D1DatabaseCorrupt,
        "D1 migration ledger is unavailable",
    )
}

#[cfg(test)]
#[path = "execution_tests.rs"]
mod tests;
