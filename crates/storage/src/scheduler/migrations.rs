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
const MIGRATION_003_SQL: &str = include_str!("../../scheduler-migrations/003_queue_consumer.sql");
const MIGRATION_004_SQL: &str = include_str!("../../scheduler-migrations/004_cron.sql");
const MIGRATION_005_SQL: &str = include_str!("../../scheduler-migrations/005_workflow_core.sql");
const MIGRATION_006_SQL: &str =
    include_str!("../../scheduler-migrations/006_workflow_durable_waiting.sql");
const MIGRATION_007_SQL: &str =
    include_str!("../../scheduler-migrations/007_workflow_operation_progress.sql");
const MIGRATION_008_SQL: &str =
    include_str!("../../scheduler-migrations/008_workflow_due_admission.sql");

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
    SchedulerMigration {
        version: 3,
        name: "003_queue_consumer",
        sql: MIGRATION_003_SQL,
        checksum: &crate::migrations::SCHEDULER_MIGRATION_003_SHA256,
    },
    SchedulerMigration {
        version: 4,
        name: "004_cron",
        sql: MIGRATION_004_SQL,
        checksum: &crate::migrations::SCHEDULER_MIGRATION_004_SHA256,
    },
    SchedulerMigration {
        version: 5,
        name: "005_workflow_core",
        sql: MIGRATION_005_SQL,
        checksum: &crate::migrations::SCHEDULER_MIGRATION_005_SHA256,
    },
    SchedulerMigration {
        version: 6,
        name: "006_workflow_durable_waiting",
        sql: MIGRATION_006_SQL,
        checksum: &crate::migrations::SCHEDULER_MIGRATION_006_SHA256,
    },
    SchedulerMigration {
        version: 7,
        name: "007_workflow_operation_progress",
        sql: MIGRATION_007_SQL,
        checksum: &crate::migrations::SCHEDULER_MIGRATION_007_SHA256,
    },
    SchedulerMigration {
        version: 8,
        name: "008_workflow_due_admission",
        sql: MIGRATION_008_SQL,
        checksum: &crate::migrations::SCHEDULER_MIGRATION_008_SHA256,
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
    let marker: Option<(i64, String)> = connection
        .query_row(
            "SELECT schema_version,data_format FROM scheduler_meta WHERE singleton=1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|_| super::corrupt())?;
    if marker != Some((schema_version, super::DATA_FORMAT.to_owned())) {
        return Err(super::corrupt());
    }
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
