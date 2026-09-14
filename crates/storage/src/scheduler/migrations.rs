//! Frozen identities of the published pre-Refinery scheduler migrations.

use open_compute_core::{ErrorCode, PlatformError};
use rusqlite::{Connection, OptionalExtension as _};

/// One published pre-Refinery scheduler migration identity.
#[derive(Clone, Copy, Debug)]
pub(super) struct SchedulerMigration {
    pub(super) version: i64,
    pub(super) name: &'static str,
    pub(super) checksum: &'static [u8; 32],
}

pub(super) const SCHEDULER_MIGRATIONS: &[SchedulerMigration] = &[
    SchedulerMigration {
        version: 1,
        name: "001_scheduler",
        checksum: &crate::migrations::SCHEDULER_MIGRATION_001_SHA256,
    },
    SchedulerMigration {
        version: 2,
        name: "002_queue_producer",
        checksum: &crate::migrations::SCHEDULER_MIGRATION_002_SHA256,
    },
    SchedulerMigration {
        version: 3,
        name: "003_queue_consumer",
        checksum: &crate::migrations::SCHEDULER_MIGRATION_003_SHA256,
    },
    SchedulerMigration {
        version: 4,
        name: "004_cron",
        checksum: &crate::migrations::SCHEDULER_MIGRATION_004_SHA256,
    },
    SchedulerMigration {
        version: 5,
        name: "005_workflow",
        checksum: &crate::migrations::SCHEDULER_MIGRATION_005_SHA256,
    },
];

pub(super) fn validate_registry(migrations: &[SchedulerMigration]) -> Result<(), PlatformError> {
    if migrations.is_empty()
        || migrations.iter().enumerate().any(|(index, migration)| {
            migration.version != i64::try_from(index + 1).unwrap_or(i64::MAX)
                || migration.name.is_empty()
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
        checksum: &CHECKSUM,
    };
    const DUPLICATE: SchedulerMigration = SchedulerMigration {
        version: 1,
        name: "duplicate",
        checksum: &CHECKSUM,
    };
    const GAP: SchedulerMigration = SchedulerMigration {
        version: 3,
        name: "gap",
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
