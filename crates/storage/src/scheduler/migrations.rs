//! Contiguous scheduler migration registry.

use open_compute_core::{ErrorCode, PlatformError};
use rusqlite::{Connection, OptionalExtension as _};

/// One immutable scheduler migration.
#[derive(Clone, Copy, Debug)]
pub(super) struct SchedulerMigration {
    pub(super) version: i64,
    pub(super) name: &'static str,
    pub(super) sql: &'static str,
    pub(super) checksum: &'static [u8; 32],
}

const MIGRATION_001_SQL: &str = include_str!("../../scheduler-migrations/001_scheduler.sql");
const MIGRATION_002_SQL: &str = include_str!("../../scheduler-migrations/002_queue_producer.sql");

pub(super) const SCHEDULER_MIGRATIONS: &[SchedulerMigration] = &[
    SchedulerMigration {
        version: 1,
        name: "001_scheduler",
        sql: MIGRATION_001_SQL,
        checksum: &crate::migrations::SCHEDULER_MIGRATION_001_SHA256,
    },
    SchedulerMigration {
        version: 2,
        name: "002_queue_producer",
        sql: MIGRATION_002_SQL,
        checksum: &crate::migrations::SCHEDULER_MIGRATION_002_SHA256,
    },
];

pub(super) fn validate_registry(migrations: &[SchedulerMigration]) -> Result<(), PlatformError> {
    if migrations.is_empty()
        || migrations.iter().enumerate().any(|(index, migration)| {
            migration.version != i64::try_from(index + 1).unwrap_or(i64::MAX)
                || migration.name.is_empty()
                || migration.sql.is_empty()
        })
    {
        return Err(PlatformError::new(
            ErrorCode::SchedulerCorrupt,
            "scheduler migration registry is not contiguous",
        ));
    }
    Ok(())
}

pub(super) fn verify_applied(
    connection: &Connection,
    schema_version: i64,
) -> Result<(), PlatformError> {
    validate_registry(SCHEDULER_MIGRATIONS)?;
    let Some(applied_count) = usize::try_from(schema_version).ok() else {
        return Err(super::corrupt());
    };
    if applied_count == 0 || applied_count > SCHEDULER_MIGRATIONS.len() {
        return Err(super::corrupt());
    }
    let count: i64 = connection
        .query_row("SELECT COUNT(*) FROM scheduler_migrations", [], |row| {
            row.get(0)
        })
        .map_err(|_| super::corrupt())?;
    if usize::try_from(count).ok() != Some(applied_count) {
        return Err(super::corrupt());
    }
    for migration in SCHEDULER_MIGRATIONS.iter().take(applied_count) {
        let applied: Option<(String, Vec<u8>)> = connection
            .query_row(
                "SELECT name, checksum_sha256
                 FROM scheduler_migrations WHERE version = ?1",
                [migration.version],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(|_| super::corrupt())?;
        if applied != Some((migration.name.to_owned(), migration.checksum.to_vec())) {
            return Err(super::corrupt());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const CHECKSUM: [u8; 32] = [0; 32];
    const ONE: SchedulerMigration = SchedulerMigration {
        version: 1,
        name: "one",
        sql: "SELECT 1;",
        checksum: &CHECKSUM,
    };
    const DUPLICATE: SchedulerMigration = SchedulerMigration {
        version: 1,
        name: "duplicate",
        sql: "SELECT 2;",
        checksum: &CHECKSUM,
    };
    const GAP: SchedulerMigration = SchedulerMigration {
        version: 3,
        name: "gap",
        sql: "SELECT 3;",
        checksum: &CHECKSUM,
    };

    #[test]
    fn registry_rejects_empty_duplicate_gap_and_reordering() {
        assert!(validate_registry(SCHEDULER_MIGRATIONS).is_ok());
        for invalid in [
            &[][..],
            &[ONE, DUPLICATE][..],
            &[ONE, GAP][..],
            &[GAP, ONE][..],
        ] {
            assert_eq!(
                validate_registry(invalid).unwrap_err().code(),
                ErrorCode::SchedulerCorrupt
            );
        }
    }
}
