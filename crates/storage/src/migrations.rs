//! Forward-only control-plane migrations.

use crate::control_db::{self, ControlDb};
use open_compute_core::clock::Clock;
use open_compute_core::{ErrorCode, PlatformError};
use rusqlite::Transaction;
use std::time::UNIX_EPOCH;

include!(concat!(env!("OUT_DIR"), "/migration_hashes.rs"));

const MIGRATION_001_SQL: &str = include_str!("../migrations/001_init.sql");
const MIGRATION_002_SQL: &str = include_str!("../migrations/002_workers_runtime.sql");
const MIGRATION_003_SQL: &str = include_str!("../migrations/003_resource_bindings.sql");
const MIGRATION_004_SQL: &str = include_str!("../migrations/004_kv.sql");
const CURRENT_VERSION: i64 = 4;
const MIGRATION_001_NAME: &str = "001_init";
const MIGRATION_002_NAME: &str = "002_workers_runtime";
const MIGRATION_003_NAME: &str = "003_resource_bindings";
const MIGRATION_004_NAME: &str = "004_kv";
const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Test-only deterministic fault injection points.
#[cfg(any(test, feature = "test-support"))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MigrationFault {
    /// Fail after `BEGIN EXCLUSIVE` and before SQL execution.
    BeforeExecution,
    /// Fail after the first DDL statement.
    DuringDdl,
    /// Fail after SQL/invariants and before the migration row write.
    BeforeMigrationRow,
    /// Fail immediately after a successful commit.
    AfterCommit,
}

/// Apply pending migrations. Never down-migrates.
pub fn apply(db: &ControlDb, clock: &dyn Clock) -> Result<(), PlatformError> {
    apply_inner(db, clock, None)
}

/// Apply pending migrations with test-only fault injection.
#[cfg(any(test, feature = "test-support"))]
pub fn apply_with_fault(
    db: &ControlDb,
    clock: &dyn Clock,
    fault: Option<MigrationFault>,
) -> Result<(), PlatformError> {
    apply_inner(db, clock, fault)
}

fn apply_inner(
    db: &ControlDb,
    clock: &dyn Clock,
    #[cfg(any(test, feature = "test-support"))] fault: Option<MigrationFault>,
    #[cfg(not(any(test, feature = "test-support")))] _fault: Option<()>,
) -> Result<(), PlatformError> {
    verify_schema_consistency(db)?;
    let user_version = db.user_version()?;
    if user_version < 1 {
        apply_one(
            db,
            clock,
            1,
            MIGRATION_001_NAME,
            MIGRATION_001_SQL,
            &MIGRATION_001_SHA256,
            #[cfg(any(test, feature = "test-support"))]
            fault,
        )?;
    }
    if db.user_version()? < 2 {
        apply_one(
            db,
            clock,
            2,
            MIGRATION_002_NAME,
            MIGRATION_002_SQL,
            &MIGRATION_002_SHA256,
            #[cfg(any(test, feature = "test-support"))]
            fault,
        )?;
    }
    if db.user_version()? < 3 {
        apply_one(
            db,
            clock,
            3,
            MIGRATION_003_NAME,
            MIGRATION_003_SQL,
            &MIGRATION_003_SHA256,
            #[cfg(any(test, feature = "test-support"))]
            fault,
        )?;
    }
    if db.user_version()? < 4 {
        apply_one(
            db,
            clock,
            4,
            MIGRATION_004_NAME,
            MIGRATION_004_SQL,
            &MIGRATION_004_SHA256,
            #[cfg(any(test, feature = "test-support"))]
            fault,
        )?;
    }
    Ok(())
}

fn verify_schema_consistency(db: &ControlDb) -> Result<(), PlatformError> {
    let user_version = db.user_version()?;
    if user_version > CURRENT_VERSION {
        return Err(PlatformError::new(
            ErrorCode::SchemaTooNew,
            "on-disk schema is newer than this binary",
        ));
    }
    let table = db.table_exists("schema_migrations")?;
    let rows = if table {
        read_applied_rows(db)?
    } else {
        Vec::new()
    };

    for (version, _) in &rows {
        if *version > CURRENT_VERSION {
            return Err(PlatformError::new(
                ErrorCode::SchemaTooNew,
                "on-disk schema is newer than this binary",
            ));
        }
    }

    if user_version == 0 {
        if !rows.is_empty() {
            return Err(PlatformError::new(
                ErrorCode::MigrationFailed,
                "user_version 0 is inconsistent with applied migration rows",
            ));
        }
        return Ok(());
    }

    if !table {
        return Err(PlatformError::new(
            ErrorCode::MigrationFailed,
            "schema_migrations is missing for a positive user_version",
        ));
    }

    let mut seen = std::collections::BTreeSet::new();
    for (version, checksum) in &rows {
        if *version > user_version {
            return Err(PlatformError::new(
                ErrorCode::MigrationFailed,
                "applied migration version is above user_version",
            ));
        }
        if *version < 1 {
            return Err(PlatformError::new(
                ErrorCode::MigrationFailed,
                "applied migration version is invalid",
            ));
        }
        if !seen.insert(*version) {
            return Err(PlatformError::new(
                ErrorCode::MigrationFailed,
                "duplicate applied migration version",
            ));
        }
        let expected = expected_checksum(*version)?;
        if checksum.as_slice() != expected {
            return Err(PlatformError::new(
                ErrorCode::MigrationFailed,
                "applied migration checksum does not match this binary",
            ));
        }
    }
    for required in 1..=user_version {
        if !seen.contains(&required) {
            return Err(PlatformError::new(
                ErrorCode::MigrationFailed,
                "applied migrations are missing a contiguous version",
            ));
        }
    }
    Ok(())
}

fn read_applied_rows(db: &ControlDb) -> Result<Vec<(i64, Vec<u8>)>, PlatformError> {
    db.with_read(|conn| {
        let mut stmt = conn
            .prepare("SELECT version, checksum_sha256 FROM schema_migrations ORDER BY version")
            .map_err(|_| {
                PlatformError::new(
                    ErrorCode::MigrationFailed,
                    "failed to read schema_migrations",
                )
            })?;
        let mapped = stmt
            .query_map([], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, Vec<u8>>(1)?))
            })
            .map_err(|_| {
                PlatformError::new(
                    ErrorCode::MigrationFailed,
                    "failed to map schema_migrations",
                )
            })?;
        let mut rows = Vec::new();
        for row in mapped {
            rows.push(row.map_err(|_| {
                PlatformError::new(
                    ErrorCode::MigrationFailed,
                    "failed to read applied migration checksum",
                )
            })?);
        }
        Ok(rows)
    })
}

pub(crate) fn expected_checksum(version: i64) -> Result<&'static [u8], PlatformError> {
    match version {
        1 => Ok(&MIGRATION_001_SHA256),
        2 => Ok(&MIGRATION_002_SHA256),
        3 => Ok(&MIGRATION_003_SHA256),
        4 => Ok(&MIGRATION_004_SHA256),
        v if v > CURRENT_VERSION => Err(PlatformError::new(
            ErrorCode::SchemaTooNew,
            "on-disk schema is newer than this binary",
        )),
        _ => Err(PlatformError::new(
            ErrorCode::MigrationFailed,
            "unknown applied migration version",
        )),
    }
}

fn apply_one(
    db: &ControlDb,
    clock: &dyn Clock,
    version: i64,
    name: &str,
    sql: &str,
    checksum: &[u8; 32],
    #[cfg(any(test, feature = "test-support"))] fault: Option<MigrationFault>,
) -> Result<(), PlatformError> {
    let applied_at_ms = millis(clock);
    let after_commit = db.with_exclusive(|tx| {
        #[cfg(any(test, feature = "test-support"))]
        if fault == Some(MigrationFault::BeforeExecution) {
            return Err(PlatformError::new(
                ErrorCode::MigrationFailed,
                "injected fault before migration execution",
            ));
        }
        // `execute_batch` is required because migrations may contain triggers,
        // whose bodies contain semicolons that are not statement boundaries.
        tx.execute_batch(sql).map_err(|_| {
            PlatformError::new(ErrorCode::MigrationFailed, "migration SQL failed")
        })?;
        #[cfg(any(test, feature = "test-support"))]
        if fault == Some(MigrationFault::DuringDdl) {
            return Err(PlatformError::new(
                ErrorCode::MigrationFailed,
                "injected fault during migration DDL",
            ));
        }
        run_invariants(tx, version)?;
        #[cfg(any(test, feature = "test-support"))]
        if fault == Some(MigrationFault::BeforeMigrationRow) {
            return Err(PlatformError::new(
                ErrorCode::MigrationFailed,
                "injected fault before migration row write",
            ));
        }
        tx.execute(
            "INSERT INTO schema_migrations (version, name, checksum_sha256, applied_at_ms, app_version)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![version, name, checksum.as_slice(), applied_at_ms, APP_VERSION],
        )
        .map_err(|_| {
            PlatformError::new(
                ErrorCode::MigrationFailed,
                "failed to insert schema_migrations row",
            )
        })?;
        control_db::set_user_version(tx, version)?;
        #[cfg(any(test, feature = "test-support"))]
        let after = fault == Some(MigrationFault::AfterCommit);
        #[cfg(not(any(test, feature = "test-support")))]
        let after = false;
        Ok(after)
    })?;
    if after_commit {
        return Err(PlatformError::new(
            ErrorCode::MigrationFailed,
            "injected fault after migration commit",
        ));
    }
    Ok(())
}

fn run_invariants(tx: &Transaction<'_>, version: i64) -> Result<(), PlatformError> {
    let mut tables = vec!["schema_migrations", "platform_meta", "accounts"];
    if version >= 2 {
        tables.extend([
            "workers",
            "worker_deployments",
            "deployment_vars",
            "deployment_secrets",
            "worker_routes",
            "control_idempotency",
            "deployment_referrers",
            "control_audit_events",
        ]);
    }
    if version >= 3 {
        tables.extend(["resources", "deployment_bindings", "resource_referrers"]);
    }
    if version >= 4 {
        tables.extend(["kv_namespaces", "kv_backups"]);
    }
    for table in tables {
        let sql: String = tx
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = ?1",
                [table],
                |row| row.get(0),
            )
            .map_err(|_| {
                PlatformError::new(
                    ErrorCode::MigrationFailed,
                    "migration invariant: required table missing",
                )
            })?;
        if !sql.to_ascii_uppercase().contains("STRICT") {
            return Err(PlatformError::new(
                ErrorCode::MigrationFailed,
                "migration invariant: table is not STRICT",
            ));
        }
    }
    let index: String = tx
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type = 'index' AND name = 'accounts_live_name'",
            [],
            |row| row.get(0),
        )
        .map_err(|_| {
            PlatformError::new(
                ErrorCode::MigrationFailed,
                "migration invariant: accounts_live_name missing",
            )
        })?;
    if !index.to_ascii_uppercase().contains("UNIQUE") || !index.contains("deleted_at_ms") {
        return Err(PlatformError::new(
            ErrorCode::MigrationFailed,
            "migration invariant: accounts_live_name is not a partial unique index",
        ));
    }
    if version >= 2 {
        for index_name in [
            "workers_live_name",
            "live_exact_routes",
            "live_platform_routes",
        ] {
            let sql: String = tx
                .query_row(
                    "SELECT sql FROM sqlite_master WHERE type = 'index' AND name = ?1",
                    [index_name],
                    |row| row.get(0),
                )
                .map_err(|_| {
                    PlatformError::new(
                        ErrorCode::MigrationFailed,
                        "migration invariant: P0.2 unique index missing",
                    )
                })?;
            if !sql.to_ascii_uppercase().contains("UNIQUE") {
                return Err(PlatformError::new(
                    ErrorCode::MigrationFailed,
                    "migration invariant: P0.2 index is not unique",
                ));
            }
        }
    }
    if version >= 3 {
        let sql: String = tx
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type = 'index' AND name = 'resources_live_name'",
                [],
                |row| row.get(0),
            )
            .map_err(|_| {
                PlatformError::new(
                    ErrorCode::MigrationFailed,
                    "migration invariant: resources_live_name missing",
                )
            })?;
        if !sql.to_ascii_uppercase().contains("UNIQUE") || !sql.contains("tombstoned") {
            return Err(PlatformError::new(
                ErrorCode::MigrationFailed,
                "migration invariant: resources_live_name is not partial unique",
            ));
        }
    }
    Ok(())
}

fn millis(clock: &dyn Clock) -> i64 {
    clock
        .now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

/// Current control-plane schema version implemented by this binary.
#[must_use]
pub fn current_schema_version() -> i64 {
    CURRENT_VERSION
}

/// Build-time SHA-256 of migration 1 SQL.
#[must_use]
pub fn migration_001_checksum() -> &'static [u8; 32] {
    &MIGRATION_001_SHA256
}

#[cfg(test)]
#[path = "migrations_tests.rs"]
mod coverage_tests;

/// Build-time SHA-256 of migration 2 SQL.
#[must_use]
pub fn migration_002_checksum() -> &'static [u8; 32] {
    &MIGRATION_002_SHA256
}

/// Build-time SHA-256 of migration 3 SQL.
#[must_use]
pub fn migration_003_checksum() -> &'static [u8; 32] {
    &MIGRATION_003_SHA256
}

/// Build-time SHA-256 of migration 4 SQL.
#[must_use]
pub fn migration_004_checksum() -> &'static [u8; 32] {
    &MIGRATION_004_SHA256
}

/// Read-only schema inspection used by doctor.
pub fn inspect_schema(db: &ControlDb) -> Result<i64, PlatformError> {
    verify_schema_consistency(db)?;
    db.user_version()
}
