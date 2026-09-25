//! Refinery-backed migration execution shared by platform-owned SQLite databases.

use open_compute_core::{ErrorCode, PlatformError};
use refinery::{Runner, Target};
use rusqlite::{Connection, OptionalExtension as _};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

pub(crate) const HISTORY_TABLE: &str = "refinery_schema_history";
type SchemaObject = (String, String, String, Option<String>);

mod control {
    refinery::embed_migrations!("refinery-migrations/control");
}

mod scheduler {
    refinery::embed_migrations!("refinery-migrations/scheduler");
}

mod observability {
    refinery::embed_migrations!("refinery-migrations/observability");
}

mod kv {
    refinery::embed_migrations!("refinery-migrations/kv");
}

mod d1 {
    refinery::embed_migrations!("refinery-migrations/d1");
}

mod vectorize {
    refinery::embed_migrations!("refinery-migrations/vectorize");
}

mod ai_search {
    refinery::embed_migrations!("refinery-migrations/ai_search");
}

/// One independently versioned physical database type.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DatabaseKind {
    Control,
    Scheduler,
    Observability,
    Kv,
    D1,
    Vectorize,
    AiSearch,
}

/// Run the embedded lineage; databases without verified history must be empty.
pub(crate) fn migrate(
    connection: &mut Connection,
    kind: DatabaseKind,
) -> Result<(), PlatformError> {
    let runner = runner(kind);
    let history_exists = table_exists(connection, HISTORY_TABLE)?;
    let allow_empty = !has_application_schema_excluding_history(connection)?;
    let applied = verify_history(connection, &runner, allow_empty)?;
    if applied > 0 {
        verify_schema_matches_version(connection, kind, applied)?;
    } else if history_exists {
        // Refinery creates its history table before it starts the first migration
        // transaction. A crash at that boundary is a clean empty database, provided the
        // table definition is exactly the one this runner would create.
        verify_schema_matches_version(connection, kind, 0)?;
    }
    let current = current_version_from_runner(&runner)?;
    if applied < current {
        runner
            .run(connection)
            .map_err(|_| migration_failed_at("Refinery rejected SQLite migration history"))?;
        let migrated = verify_history(connection, &runner, false)?;
        if migrated != current {
            return Err(migration_failed());
        }
        verify_schema_matches_version(connection, kind, current)?;
    }
    Ok(())
}

/// Validate an existing database's complete Refinery history without applying migrations.
pub(crate) fn inspect(
    connection: &mut Connection,
    kind: DatabaseKind,
) -> Result<i64, PlatformError> {
    let runner = runner(kind);
    let applied = verify_history(connection, &runner, false)?;
    verify_schema_matches_version(connection, kind, applied)?;
    Ok(applied)
}

/// Embedded head for one database lineage.
pub(crate) fn current_version(kind: DatabaseKind) -> i64 {
    runner(kind)
        .get_migrations()
        .iter()
        .map(|migration| i64::from(migration.version()))
        .max()
        .unwrap_or(0)
}

fn runner(kind: DatabaseKind) -> Runner {
    let runner = match kind {
        DatabaseKind::Control => control::migrations::runner(),
        DatabaseKind::Scheduler => scheduler::migrations::runner(),
        DatabaseKind::Observability => observability::migrations::runner(),
        DatabaseKind::Kv => kv::migrations::runner(),
        DatabaseKind::D1 => d1::migrations::runner(),
        DatabaseKind::Vectorize => vectorize::migrations::runner(),
        DatabaseKind::AiSearch => ai_search::migrations::runner(),
    };
    // SQLite must commit migration SQL and its history together, including on SIGKILL.
    runner.set_grouped(true)
}

#[cfg(test)]
pub(crate) fn migrate_to_for_test(connection: &mut Connection, kind: DatabaseKind, version: i32) {
    runner(kind)
        .set_target(Target::Version(version))
        .run(connection)
        .unwrap();
}

fn verify_schema_matches_version(
    actual: &Connection,
    kind: DatabaseKind,
    version: i64,
) -> Result<(), PlatformError> {
    let version = i32::try_from(version).map_err(|_| migration_failed())?;
    let mut expected = Connection::open_in_memory()
        .map_err(|_| migration_failed_at("could not create schema verifier"))?;
    runner(kind)
        .set_target(Target::Version(version))
        .run(&mut expected)
        .map_err(|_| migration_failed_at("embedded migration lineage is invalid"))?;
    let actual = schema_signature(actual, kind)
        .map_err(|_| migration_failed_at("could not inspect SQLite schema"))?;
    let expected = schema_signature(&expected, kind)
        .map_err(|_| migration_failed_at("could not inspect embedded schema"))?;
    if actual == expected {
        Ok(())
    } else {
        Err(migration_failed_at(
            "SQLite schema does not exactly match its embedded migration head",
        ))
    }
}

fn schema_signature(
    connection: &Connection,
    kind: DatabaseKind,
) -> Result<Vec<SchemaObject>, PlatformError> {
    let mut statement = connection
        .prepare(
            "SELECT type, name, tbl_name, sql FROM sqlite_master
             WHERE type IN ('table','index','view','trigger')
               AND name NOT LIKE 'sqlite_%'
             ORDER BY type, name",
        )
        .map_err(|_| migration_failed())?;
    statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
            ))
        })
        .map_err(|_| migration_failed())?
        .filter_map(|row| match row {
            Ok((object_type, name, table, sql))
                if kind != DatabaseKind::D1
                    || name == HISTORY_TABLE
                    || name.starts_with("__open_compute_")
                    || table.starts_with("__open_compute_") =>
            {
                Some(Ok((
                    object_type,
                    name,
                    table,
                    sql.map(|value| normalize_sql(&value)),
                )))
            }
            Ok(_) => None,
            Err(_) => Some(Err(migration_failed())),
        })
        .collect()
}

fn normalize_sql(sql: &str) -> String {
    let mut normalized = String::with_capacity(sql.len());
    let mut quote = None;
    let mut characters = sql.chars().peekable();
    while let Some(character) = characters.next() {
        if quote.is_none() && character == '/' && characters.peek() == Some(&'*') {
            let _ = characters.next();
            while let Some(comment) = characters.next() {
                if comment == '*' && characters.peek() == Some(&'/') {
                    let _ = characters.next();
                    break;
                }
            }
            continue;
        }
        if matches!(character, '\'' | '"') {
            if quote == Some(character) {
                quote = None;
            } else if quote.is_none() {
                quote = Some(character);
            }
            normalized.push(character);
        } else if quote.is_some() || !character.is_whitespace() {
            normalized.push(character);
        }
    }
    normalized
}

fn verify_history(
    connection: &Connection,
    runner: &Runner,
    allow_empty: bool,
) -> Result<i64, PlatformError> {
    if !table_exists(connection, HISTORY_TABLE)? {
        return if allow_empty {
            Ok(0)
        } else {
            Err(migration_failed())
        };
    }
    let mut statement = connection
        .prepare(
            "SELECT version, name, applied_on, checksum
             FROM refinery_schema_history ORDER BY version",
        )
        .map_err(|_| migration_failed())?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })
        .map_err(|_| migration_failed())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| migration_failed())?;
    if rows.is_empty() {
        return if allow_empty {
            Ok(0)
        } else {
            Err(migration_failed())
        };
    }
    let current = current_version_from_runner(runner)?;
    for (index, (version, name, applied_on, checksum)) in rows.iter().enumerate() {
        if *version > current {
            return Err(PlatformError::new(
                ErrorCode::SchemaTooNew,
                "SQLite migration history is newer than this binary",
            ));
        }
        let expected = runner
            .get_migrations()
            .iter()
            .find(|migration| i64::from(migration.version()) == *version)
            .ok_or_else(migration_failed)?;
        if *version != i64::try_from(index + 1).map_err(|_| migration_failed())?
            || *version != i64::from(expected.version())
            || name != expected.name()
            || checksum.parse::<u64>().ok() != Some(expected.checksum())
            || OffsetDateTime::parse(applied_on, &Rfc3339).is_err()
        {
            return Err(migration_failed());
        }
    }
    rows.last()
        .map(|(version, _, _, _)| *version)
        .ok_or_else(migration_failed)
}

fn current_version_from_runner(runner: &Runner) -> Result<i64, PlatformError> {
    runner
        .get_migrations()
        .iter()
        .map(|migration| i64::from(migration.version()))
        .max()
        .ok_or_else(migration_failed)
}

fn table_exists(connection: &Connection, table: &str) -> Result<bool, PlatformError> {
    connection
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1",
            [table],
            |_| Ok(()),
        )
        .optional()
        .map(|row| row.is_some())
        .map_err(|_| migration_failed())
}

fn has_application_schema_excluding_history(
    connection: &Connection,
) -> Result<bool, PlatformError> {
    connection
        .query_row(
            "SELECT EXISTS(
               SELECT 1 FROM sqlite_master
               WHERE type IN ('table','view','trigger')
                 AND name NOT LIKE 'sqlite_%' AND name != ?1
             )",
            [HISTORY_TABLE],
            |row| row.get(0),
        )
        .map_err(|_| migration_failed())
}

fn migration_failed() -> PlatformError {
    migration_failed_at("history or schema does not match this binary")
}

fn migration_failed_at(reason: &'static str) -> PlatformError {
    PlatformError::new(ErrorCode::MigrationFailed, reason)
}

#[cfg(test)]
mod tests {
    use super::*;
    use refinery::Migration;

    #[test]
    fn migration_history_is_verified_by_version_not_discovery_order() {
        let v1 = Migration::unapplied("V1__init", "CREATE TABLE item(id INTEGER);").unwrap();
        let v2 =
            Migration::unapplied("V2__extend", "ALTER TABLE item ADD COLUMN name TEXT;").unwrap();
        let runner = Runner::new(&[v2, v1]);
        let mut connection = Connection::open_in_memory().unwrap();

        runner.run(&mut connection).unwrap();

        assert_eq!(current_version_from_runner(&runner).unwrap(), 2);
        assert_eq!(verify_history(&connection, &runner, false).unwrap(), 2);
    }
}
