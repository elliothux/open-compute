//! SQLite Online Backup and restore-as-new identity rewriting.

use super::D1DatabaseRecord;
use super::engine::{
    D1_DATABASE_SCHEMA_VERSION, D1Engine, identity_error, map_open_error, read_session_version,
};
use crate::fs;
use open_compute_core::{AccountId, ErrorCode, PlatformError, ResourceId};
use rusqlite::{Connection, MAIN_DB, OpenFlags, params};
use std::path::Path;

impl D1Engine {
    /// Verify a completed snapshot without opening it for mutation or creating sidecars.
    pub fn verify_completed_snapshot(
        snapshot: &Path,
        record: &D1DatabaseRecord,
        session_version: u64,
    ) -> Result<(), PlatformError> {
        if record.schema_version != D1_DATABASE_SCHEMA_VERSION
            || record.resource.driver_schema_version != D1_DATABASE_SCHEMA_VERSION
            || !snapshot.is_file()
        {
            return Err(identity_error());
        }
        fs::validate_owned_file(snapshot, true).map_err(|_| identity_error())?;
        let connection = Connection::open_with_flags(
            snapshot,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(|error| map_open_error(&error))?;
        for (key, expected) in [
            ("format", "open-compute-d1".to_owned()),
            ("schema_version", D1_DATABASE_SCHEMA_VERSION.to_string()),
            ("resource_id", record.resource.id.to_string()),
            ("account_id", record.resource.account_id.to_string()),
        ] {
            let actual: Vec<u8> = connection
                .query_row(
                    "SELECT value FROM __open_compute_meta WHERE key = ?1",
                    [key],
                    |row| row.get(0),
                )
                .map_err(|_| identity_error())?;
            if actual != expected.as_bytes() {
                return Err(identity_error());
            }
        }
        if read_session_version(&connection)? != session_version {
            return Err(identity_error());
        }
        let check: String = connection
            .query_row("PRAGMA quick_check(1)", [], |row| row.get(0))
            .map_err(|error| map_open_error(&error))?;
        if check != "ok" {
            return Err(PlatformError::new(
                ErrorCode::D1DatabaseCorrupt,
                "D1 snapshot failed integrity validation",
            ));
        }
        Ok(())
    }

    /// Create a transactionally consistent standalone snapshot.
    pub fn online_backup(&self, destination: &Path) -> Result<(), PlatformError> {
        if destination.exists() || std::fs::symlink_metadata(destination).is_ok() {
            return Err(backup_error());
        }
        let source = self.open()?;
        source
            .backup(MAIN_DB, destination, None)
            .map_err(|error| map_open_error(&error))?;
        fs::chmod(destination, 0o600)?;
        sync_database(destination)?;
        let snapshot = Self {
            path: destination.to_path_buf(),
            account_id: self.account_id,
            resource_id: self.resource_id,
            quota_bytes: self.quota_bytes,
        };
        snapshot.verify_identity()?;
        snapshot.quick_check()
    }

    /// Restore a verified snapshot into a new unpublished resource identity.
    pub fn restore_as_new(
        snapshot: &Path,
        destination: &Path,
        account_id: AccountId,
        resource_id: ResourceId,
        created_at_ms: i64,
        quota_bytes: u64,
    ) -> Result<Self, PlatformError> {
        if destination.exists()
            || std::fs::symlink_metadata(destination).is_ok()
            || !snapshot.is_file()
            || quota_bytes < 64 * 1024 * 1024
        {
            return Err(backup_error());
        }
        fs::validate_owned_file(snapshot, true).map_err(|_| backup_error())?;
        let source = Connection::open_with_flags(
            snapshot,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(|error| map_open_error(&error))?;
        let check: String = source
            .query_row("PRAGMA quick_check(1)", [], |row| row.get(0))
            .map_err(|error| map_open_error(&error))?;
        if check != "ok" {
            return Err(PlatformError::new(
                ErrorCode::D1DatabaseCorrupt,
                "D1 backup failed integrity validation",
            ));
        }
        source
            .backup(MAIN_DB, destination, None)
            .map_err(|error| map_open_error(&error))?;
        drop(source);
        fs::chmod(destination, 0o600)?;
        let connection = Connection::open_with_flags(
            destination,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(|error| map_open_error(&error))?;
        super::hardening::configure_connection(&connection, quota_bytes)?;
        let transaction = connection
            .unchecked_transaction()
            .map_err(|error| map_open_error(&error))?;
        for (key, value) in [
            ("account_id", account_id.to_string()),
            ("resource_id", resource_id.to_string()),
            ("created_at_ms", created_at_ms.to_string()),
        ] {
            if transaction
                .execute(
                    "UPDATE __open_compute_meta SET value = ?1 WHERE key = ?2",
                    params![value.as_bytes(), key],
                )
                .map_err(|error| map_open_error(&error))?
                != 1
            {
                return Err(identity_error());
            }
        }
        transaction
            .commit()
            .map_err(|error| map_open_error(&error))?;
        connection
            .execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")
            .map_err(|error| map_open_error(&error))?;
        drop(connection);
        sync_database(destination)?;
        let engine = Self {
            path: destination.to_path_buf(),
            account_id,
            resource_id,
            quota_bytes,
        };
        engine.verify_identity()?;
        engine.quick_check()?;
        Ok(engine)
    }
}

fn backup_error() -> PlatformError {
    PlatformError::new(
        ErrorCode::D1InternalProtocolError,
        "D1 backup staging invariant failed",
    )
}

fn sync_database(path: &Path) -> Result<(), PlatformError> {
    let file = fs::open_nofollow(path, false, true)?;
    fs::validate_authority_fd(&file)?;
    file.sync_all().map_err(|_| backup_error())?;
    drop(file);
    fs::fsync_dir(path.parent().ok_or_else(backup_error)?)
}

#[cfg(test)]
#[path = "backup_tests.rs"]
mod tests;
