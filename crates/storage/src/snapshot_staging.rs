//! Exact-layout cleanup for crash-retained local platform snapshot staging.

use crate::DataDir;
use open_compute_core::{ErrorCode, PlatformError};
use std::time::SystemTime;

const MAX_STAGING_DIRECTORIES: usize = 100_000;
const MAX_FILES_PER_DIRECTORY: usize = 1_000_000;

/// Aggregate local staging bytes reclaimed from authenticated layout names.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct LocalSnapshotStagingCleanup {
    /// Exact platform snapshot staging directories removed.
    pub directories: u64,
    /// Exact flat staging files removed.
    pub files: u64,
    /// Filesystem-reported bytes removed.
    pub bytes: u64,
}

/// Remove only canonical `platform-<uuidv7>` staging directories older than `deadline`.
pub fn cleanup_stale_snapshot_staging(
    data_dir: &DataDir,
    deadline: SystemTime,
) -> Result<LocalSnapshotStagingCleanup, PlatformError> {
    let root = data_dir.backup_staging_dir();
    crate::fs::validate_owned_dir(&root)?;
    let mut cleanup = LocalSnapshotStagingCleanup::default();
    let entries = std::fs::read_dir(&root).map_err(|_| staging_invalid())?;
    for (index, entry) in entries.enumerate() {
        if index >= MAX_STAGING_DIRECTORIES {
            return Err(staging_invalid());
        }
        let entry = entry.map_err(|_| staging_invalid())?;
        let name = entry.file_name();
        let Some(snapshot_id) = name
            .to_str()
            .and_then(|value| value.strip_prefix("platform-"))
            .filter(|value| canonical_uuid_v7(value))
        else {
            continue;
        };
        let _ = snapshot_id;
        let kind = entry.file_type().map_err(|_| staging_invalid())?;
        if kind.is_symlink() || !kind.is_dir() {
            return Err(staging_invalid());
        }
        crate::fs::validate_owned_dir(&entry.path())?;
        let modified = entry
            .metadata()
            .and_then(|metadata| metadata.modified())
            .map_err(|_| staging_invalid())?;
        if modified > deadline {
            continue;
        }
        let (files, bytes) = validate_flat_directory(&entry.path())?;
        for file in files {
            std::fs::remove_file(file).map_err(|_| staging_invalid())?;
        }
        std::fs::remove_dir(entry.path()).map_err(|_| staging_invalid())?;
        cleanup.directories = cleanup.directories.saturating_add(1);
        cleanup.files = cleanup.files.saturating_add(bytes.0);
        cleanup.bytes = cleanup.bytes.saturating_add(bytes.1);
    }
    if cleanup.directories > 0 {
        crate::fs::fsync_dir(&root)?;
    }
    Ok(cleanup)
}

fn validate_flat_directory(
    root: &std::path::Path,
) -> Result<(Vec<std::path::PathBuf>, (u64, u64)), PlatformError> {
    let mut paths = Vec::new();
    let mut count = 0_u64;
    let mut bytes = 0_u64;
    for (index, entry) in std::fs::read_dir(root)
        .map_err(|_| staging_invalid())?
        .enumerate()
    {
        if index >= MAX_FILES_PER_DIRECTORY {
            return Err(staging_invalid());
        }
        let entry = entry.map_err(|_| staging_invalid())?;
        let kind = entry.file_type().map_err(|_| staging_invalid())?;
        let valid_name = entry.file_name().to_str().is_some_and(|name| {
            name.len() == 10
                && name.ends_with(".bin")
                && name[..6].bytes().all(|byte| byte.is_ascii_digit())
        });
        if kind.is_symlink() || !kind.is_file() || !valid_name {
            return Err(staging_invalid());
        }
        crate::fs::validate_owned_file(&entry.path(), true)?;
        let size = entry.metadata().map_err(|_| staging_invalid())?.len();
        count = count.saturating_add(1);
        bytes = bytes.checked_add(size).ok_or_else(staging_invalid)?;
        paths.push(entry.path());
    }
    Ok((paths, (count, bytes)))
}

fn canonical_uuid_v7(value: &str) -> bool {
    uuid::Uuid::parse_str(value)
        .ok()
        .is_some_and(|id| id.get_version_num() == 7 && id.hyphenated().to_string() == value)
}

fn staging_invalid() -> PlatformError {
    PlatformError::new(
        ErrorCode::SnapshotInvalid,
        "local platform snapshot staging failed validation",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PlatformStorage;
    use open_compute_core::{StorageConfig, SystemClock};
    use std::fs;
    use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};

    #[test]
    fn cleanup_removes_only_old_canonical_flat_snapshot_staging() {
        let temp = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(temp.path()).unwrap().join("data");
        let config = StorageConfig {
            data_dir: root.clone(),
            master_key_file: root.join("keys/master.key"),
            master_key_env: None,
            sqlite_busy_timeout_ms: 5_000,
            free_space_soft_bytes: 1_073_741_824,
            free_space_hard_bytes: 268_435_456,
        };
        let storage = PlatformStorage::bootstrap(&config, &SystemClock).unwrap();
        let backup_root = storage.data_dir().backup_staging_dir();
        let snapshot = uuid::Uuid::now_v7().hyphenated().to_string();
        let owned = backup_root.join(format!("platform-{snapshot}"));
        fs::create_dir(&owned).unwrap();
        fs::set_permissions(&owned, fs::Permissions::from_mode(0o700)).unwrap();
        let file = owned.join("000000.bin");
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&file)
            .unwrap();
        fs::write(&file, b"owned").unwrap();
        let unknown = backup_root.join("operator-retained");
        fs::create_dir(&unknown).unwrap();

        let result = cleanup_stale_snapshot_staging(storage.data_dir(), SystemTime::now()).unwrap();
        assert_eq!(result.directories, 1);
        assert_eq!(result.files, 1);
        assert_eq!(result.bytes, 5);
        assert!(!owned.exists());
        assert!(unknown.exists());
    }

    #[test]
    fn cleanup_refuses_symlink_or_noncanonical_content_in_owned_layout() {
        let temp = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(temp.path()).unwrap().join("data");
        let config = StorageConfig {
            data_dir: root.clone(),
            master_key_file: root.join("keys/master.key"),
            master_key_env: None,
            sqlite_busy_timeout_ms: 5_000,
            free_space_soft_bytes: 1_073_741_824,
            free_space_hard_bytes: 268_435_456,
        };
        let storage = PlatformStorage::bootstrap(&config, &SystemClock).unwrap();
        let snapshot = uuid::Uuid::now_v7().hyphenated().to_string();
        let owned = storage
            .data_dir()
            .backup_staging_dir()
            .join(format!("platform-{snapshot}"));
        fs::create_dir(&owned).unwrap();
        fs::set_permissions(&owned, fs::Permissions::from_mode(0o700)).unwrap();
        std::os::unix::fs::symlink(temp.path(), owned.join("000000.bin")).unwrap();

        assert_eq!(
            cleanup_stale_snapshot_staging(storage.data_dir(), SystemTime::now())
                .unwrap_err()
                .code(),
            ErrorCode::SnapshotInvalid
        );
        assert!(owned.exists());
    }
}
