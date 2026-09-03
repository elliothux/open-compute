//! Verified SQL export and atomically fenced import primitives.

use super::engine::{bump_session_version, limit_error};
use super::execution::execute_tail_batch;
use super::hardening::{SqlAuthority, install_guard, remove_guard};
use super::{D1DatabaseRecord, D1Engine, D1QueryLimits};
use open_compute_core::{ErrorCode, PlatformError};
use rusqlite::types::ValueRef;
use rusqlite::{Connection, OpenFlags};
use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::Path;
use std::time::Instant;

/// Validated Cloudflare D1 SQL export selection.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct D1ExportOptions {
    /// Omit table, index, trigger, view, and user-version schema statements.
    pub no_schema: bool,
    /// Omit table row inserts.
    pub no_data: bool,
    /// Export only these tables; an empty set selects every user table.
    pub tables: BTreeSet<String>,
}

/// Durable result metadata captured before an import commit.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct D1ImportResult {
    /// Statements executed from the SQL tail.
    pub num_queries: u64,
    /// SQLite execution wall time in milliseconds.
    pub duration_ms: f64,
    /// Rows returned and consumed by imported statements.
    pub rows_read: u64,
    /// Rows changed by SQLite statements.
    pub rows_written: u64,
    /// Logical database bytes after the imported transaction.
    pub size_after: u64,
}

impl D1Engine {
    /// Render one verified completed snapshot as bounded, importable SQLite SQL.
    pub fn export_sql(
        snapshot: &Path,
        record: &D1DatabaseRecord,
        session_version: u64,
        options: &D1ExportOptions,
        max_bytes: usize,
    ) -> Result<Vec<u8>, PlatformError> {
        if options.no_schema && options.no_data {
            return Err(sql_error());
        }
        Self::verify_completed_snapshot(snapshot, record, session_version)?;
        let connection = Connection::open_with_flags(
            snapshot,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(|_| transfer_error())?;
        let shadow_tables = shadow_tables(&connection)?;
        let mut schema = connection
            .prepare(
                "SELECT type, name, tbl_name, sql FROM sqlite_schema
                 WHERE sql IS NOT NULL AND name NOT LIKE 'sqlite_%'
                   AND name NOT LIKE '__open_compute_%'
                 ORDER BY CASE type WHEN 'table' THEN 0 WHEN 'view' THEN 1
                          WHEN 'index' THEN 2 ELSE 3 END, name",
            )
            .map_err(|_| transfer_error())?;
        let objects = schema
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })
            .map_err(|_| transfer_error())?
            .map(|row| row.map_err(|_| transfer_error()))
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .filter(|(_, name, table, _)| {
                !shadow_tables.contains(name)
                    && (options.tables.is_empty() || options.tables.contains(table))
            })
            .collect::<Vec<_>>();
        drop(schema);

        if !options.tables.is_empty() {
            let selected = objects
                .iter()
                .filter(|(kind, _, _, _)| kind == "table")
                .map(|(_, name, _, _)| name)
                .collect::<BTreeSet<_>>();
            if options.tables.iter().any(|table| !selected.contains(table)) {
                return Err(sql_error());
            }
        }

        let mut output = String::new();
        if !options.no_schema {
            for (_, _, _, sql) in objects.iter().filter(|(kind, _, _, _)| kind == "table") {
                append_sql(&mut output, sql, max_bytes)?;
            }
        }
        if !options.no_data {
            for (_, table, _, _) in objects.iter().filter(|(kind, _, _, _)| kind == "table") {
                append_table_rows(&connection, table, &mut output, max_bytes)?;
            }
        }
        if !options.no_schema {
            for (_, _, _, sql) in objects.iter().filter(|(kind, _, _, _)| kind != "table") {
                append_sql(&mut output, sql, max_bytes)?;
            }
        }
        let user_version: u32 = connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .map_err(|_| transfer_error())?;
        if !options.no_schema && user_version != 0 {
            append_sql(
                &mut output,
                &format!("PRAGMA user_version = {user_version}"),
                max_bytes,
            )?;
        }
        if output.is_empty() {
            output.push_str("SELECT 1;\n");
        }
        Ok(output.into_bytes())
    }

    /// Apply SQL atomically, persisting external ingest evidence before commit.
    pub fn import_sql<F>(
        &self,
        sql: &str,
        limits: D1QueryLimits,
        before_commit: F,
    ) -> Result<D1ImportResult, PlatformError>
    where
        F: FnOnce(D1ImportResult) -> Result<(), PlatformError>,
    {
        if sql.trim().is_empty() || sql.len() > super::D1_MAX_TRANSFER_SQL_BYTES {
            return Err(sql_error());
        }
        let connection = self.open()?;
        let started = Instant::now();
        let changes_before = connection.total_changes();
        connection
            .execute_batch("BEGIN IMMEDIATE")
            .map_err(|_| transfer_error())?;
        if connection
            .execute_batch("PRAGMA defer_foreign_keys = ON")
            .is_err()
        {
            let _ = connection.execute_batch("ROLLBACK");
            return Err(transfer_error());
        }
        let control = install_guard(&connection, limits, SqlAuthority::Migration);
        let execution =
            execute_tail_batch(&connection, sql, super::D1_MAX_EXEC_STATEMENTS, &control);
        remove_guard(&connection);
        let (count, rows_read) = match execution {
            Ok((count, _, rows_read)) if count != 0 => (count, rows_read),
            Ok(_) => {
                let _ = connection.execute_batch("ROLLBACK");
                return Err(sql_error());
            }
            Err(error) => {
                let _ = connection.execute_batch("ROLLBACK");
                return Err(error);
            }
        };
        let result = D1ImportResult {
            num_queries: u64::try_from(count).map_err(|_| limit_error())?,
            duration_ms: started.elapsed().as_secs_f64() * 1000.0,
            rows_read,
            rows_written: connection.total_changes().saturating_sub(changes_before),
            size_after: super::execution::logical_size(&connection)?,
        };
        if let Err(error) = before_commit(result) {
            let _ = connection.execute_batch("ROLLBACK");
            return Err(error);
        }
        if let Err(error) = bump_session_version(&connection) {
            let _ = connection.execute_batch("ROLLBACK");
            return Err(error);
        }
        if connection.execute_batch("COMMIT").is_err() {
            let _ = connection.execute_batch("ROLLBACK");
            return Err(PlatformError::new(
                ErrorCode::D1ResultUnknown,
                "D1 import commit result is unknown",
            ));
        }
        Ok(result)
    }
}

fn shadow_tables(connection: &Connection) -> Result<BTreeSet<String>, PlatformError> {
    let mut statement = connection
        .prepare("PRAGMA table_list")
        .map_err(|_| transfer_error())?;
    statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(1)?, row.get::<_, String>(2)?))
        })
        .map_err(|_| transfer_error())?
        .filter_map(|row| match row {
            Ok((name, kind)) if kind == "shadow" => Some(Ok(name)),
            Ok(_) => None,
            Err(_) => Some(Err(transfer_error())),
        })
        .collect()
}

fn append_table_rows(
    connection: &Connection,
    table: &str,
    output: &mut String,
    max_bytes: usize,
) -> Result<(), PlatformError> {
    let identifier = quote_identifier(table);
    let mut columns = connection
        .prepare(&format!("PRAGMA table_xinfo({identifier})"))
        .map_err(|_| transfer_error())?;
    let columns = columns
        .query_map([], |row| {
            Ok((row.get::<_, String>(1)?, row.get::<_, i64>(6)?))
        })
        .map_err(|_| transfer_error())?
        .filter_map(|row| match row {
            Ok((name, 0)) => Some(Ok(name)),
            Ok(_) => None,
            Err(_) => Some(Err(transfer_error())),
        })
        .collect::<Result<Vec<_>, _>>()?;
    if columns.is_empty() {
        return Ok(());
    }
    let quoted_columns = columns
        .iter()
        .map(|column| quote_identifier(column))
        .collect::<Vec<_>>();
    let mut rows = connection
        .prepare(&format!(
            "SELECT {} FROM {identifier}",
            quoted_columns.join(",")
        ))
        .map_err(|_| transfer_error())?;
    let mut rows = rows.query([]).map_err(|_| transfer_error())?;
    while let Some(row) = rows.next().map_err(|_| transfer_error())? {
        let mut values = Vec::with_capacity(columns.len());
        for index in 0..columns.len() {
            values.push(sql_literal(
                row.get_ref(index).map_err(|_| transfer_error())?,
            ));
        }
        append_sql(
            output,
            &format!(
                "INSERT INTO {identifier} ({}) VALUES ({})",
                quoted_columns.join(","),
                values.join(",")
            ),
            max_bytes,
        )?;
    }
    Ok(())
}

fn sql_literal(value: ValueRef<'_>) -> String {
    match value {
        ValueRef::Null => "NULL".to_owned(),
        ValueRef::Integer(value) => value.to_string(),
        ValueRef::Real(value) if value.is_infinite() && value.is_sign_positive() => {
            "9.0e999".to_owned()
        }
        ValueRef::Real(value) if value.is_infinite() => "-9.0e999".to_owned(),
        ValueRef::Real(value) if value.is_nan() => "NULL".to_owned(),
        ValueRef::Real(value) => format!("{value:.17e}"),
        ValueRef::Text(value) => format!("CAST(X'{}' AS TEXT)", hex::encode(value)),
        ValueRef::Blob(value) => format!("X'{}'", hex::encode(value)),
    }
}

fn append_sql(output: &mut String, sql: &str, max_bytes: usize) -> Result<(), PlatformError> {
    let additional = sql.len().checked_add(2).ok_or_else(limit_error)?;
    if output
        .len()
        .checked_add(additional)
        .is_none_or(|size| size > max_bytes)
    {
        return Err(limit_error());
    }
    writeln!(output, "{};", sql.trim_end_matches(';')).map_err(|_| transfer_error())
}

fn quote_identifier(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}

fn sql_error() -> PlatformError {
    PlatformError::new(ErrorCode::D1SqlInvalid, "D1 import SQL is invalid")
}

fn transfer_error() -> PlatformError {
    PlatformError::new(
        ErrorCode::D1DatabaseCorrupt,
        "D1 SQL transfer could not be verified",
    )
}

#[cfg(test)]
#[path = "transfer_tests.rs"]
mod tests;
