//! Exact cleanup for object bytes retained by a failed fresh-host restore.

use open_compute_core::{ErrorCode, PlatformError, valid_restore_path};
use rustix::fs::{FlockOperation, Mode, OFlags, flock};
use std::fs::File;
use std::os::unix::fs::MetadataExt as _;
use std::path::{Component, Path, PathBuf};
use uuid::Uuid;

/// Aggregate bytes reclaimed from one exact failed-restore staging identity.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
pub struct RestoreStagingCleanup {
    /// Result schema version.
    pub schema_version: u32,
    /// Canonical `UUIDv7` staging identity selected by the operator.
    pub staging_id: String,
    /// Exact regular files removed.
    pub files: u64,
    /// Filesystem-reported bytes removed.
    pub bytes: u64,
}

/// Remove one exact `.TARGET.restore-UUIDv7` tree after fail-closed validation.
pub fn cleanup_restore_staging(
    target: &Path,
    staging_id: &str,
    max_files: u32,
    max_file_bytes: u64,
    max_total_bytes: u64,
) -> Result<RestoreStagingCleanup, PlatformError> {
    if !canonical_uuid_v7(staging_id)
        || max_files == 0
        || max_file_bytes == 0
        || max_total_bytes < max_file_bytes
    {
        return Err(cleanup_invalid());
    }
    let (parent, target_name) = validate_target_parent(target)?;
    let lock = acquire_restore_lock(&parent, &target_name)?;
    let staging = parent.join(format!(".{target_name}.restore-{staging_id}"));
    let receipt = parent.join(format!(".{target_name}.restore-failure-{staging_id}.json"));
    let (mut files, mut directories, bytes) =
        validate_staging_tree(&staging, max_files, max_file_bytes, max_total_bytes)?;
    let receipt_exists = validate_optional_receipt(&receipt)?;
    files.sort();
    for file in files.iter().rev() {
        std::fs::remove_file(file).map_err(|_| cleanup_invalid())?;
    }
    directories.sort_by_key(|path| std::cmp::Reverse(path.components().count()));
    for directory in directories {
        std::fs::remove_dir(directory).map_err(|_| cleanup_invalid())?;
    }
    if receipt_exists {
        std::fs::remove_file(&receipt).map_err(|_| cleanup_invalid())?;
    }
    crate::fs::fsync_dir(&parent)?;
    let _ = flock(&lock, FlockOperation::Unlock);
    Ok(RestoreStagingCleanup {
        schema_version: 1,
        staging_id: staging_id.to_owned(),
        files: files.len() as u64,
        bytes,
    })
}

fn validate_optional_receipt(path: &Path) -> Result<bool, PlatformError> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata)
            if metadata.file_type().is_file()
                && !metadata.file_type().is_symlink()
                && metadata.nlink() == 1 =>
        {
            crate::fs::validate_owned_file(path, true).map_err(|_| cleanup_invalid())?;
            Ok(true)
        }
        Ok(_) => Err(cleanup_invalid()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(_) => Err(cleanup_invalid()),
    }
}

pub(crate) fn record_restore_failure(
    target: &Path,
    parent: &Path,
    staging_id: &str,
) -> Result<(), PlatformError> {
    let target_name = target
        .file_name()
        .and_then(|value| value.to_str())
        .filter(|value| valid_target_name(value))
        .ok_or_else(cleanup_invalid)?;
    if !canonical_uuid_v7(staging_id) {
        return Err(cleanup_invalid());
    }
    let receipt = parent.join(format!(".{target_name}.restore-failure-{staging_id}.json"));
    let created_at_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
        .unwrap_or(i64::MAX);
    let bytes = serde_json::to_vec(&serde_json::json!({
        "schema_version": 1,
        "staging_id": staging_id,
        "target_name": target_name,
        "created_at_ms": created_at_ms,
        "object_bytes_retained": true,
        "cleanup_command": "backup cleanup-restore --staging <uuidv7>",
    }))
    .map_err(|_| cleanup_invalid())?;
    crate::fs::atomic_write(&receipt, &bytes)
}

fn validate_staging_tree(
    staging: &Path,
    max_files: u32,
    max_file_bytes: u64,
    max_total_bytes: u64,
) -> Result<(Vec<PathBuf>, Vec<PathBuf>, u64), PlatformError> {
    crate::fs::validate_owned_dir(staging).map_err(|_| cleanup_invalid())?;
    let mut pending = vec![staging.to_path_buf()];
    let mut directories = Vec::new();
    let mut files = Vec::new();
    let mut total = 0_u64;
    while let Some(directory) = pending.pop() {
        directories.push(directory.clone());
        for entry in std::fs::read_dir(&directory).map_err(|_| cleanup_invalid())? {
            let entry = entry.map_err(|_| cleanup_invalid())?;
            let metadata =
                std::fs::symlink_metadata(entry.path()).map_err(|_| cleanup_invalid())?;
            if metadata.file_type().is_symlink() {
                return Err(cleanup_invalid());
            }
            if metadata.is_dir() {
                crate::fs::validate_owned_dir(&entry.path()).map_err(|_| cleanup_invalid())?;
                pending.push(entry.path());
            } else if metadata.is_file() {
                if metadata.nlink() != 1
                    || files.len() >= max_files as usize
                    || metadata.len() > max_file_bytes
                {
                    return Err(cleanup_invalid());
                }
                crate::fs::validate_owned_file(&entry.path(), true)
                    .map_err(|_| cleanup_invalid())?;
                let path = entry.path();
                let relative = path
                    .strip_prefix(staging)
                    .ok()
                    .and_then(Path::to_str)
                    .ok_or_else(cleanup_invalid)?;
                if !valid_restore_path(relative) {
                    return Err(cleanup_invalid());
                }
                total = total
                    .checked_add(metadata.len())
                    .filter(|value| *value <= max_total_bytes)
                    .ok_or_else(cleanup_invalid)?;
                files.push(path);
            } else {
                return Err(cleanup_invalid());
            }
        }
    }
    Ok((files, directories, total))
}

fn validate_target_parent(target: &Path) -> Result<(PathBuf, String), PlatformError> {
    crate::fs::require_absolute(target)?;
    if target
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(cleanup_invalid());
    }
    let parent = target.parent().ok_or_else(cleanup_invalid)?;
    let canonical_parent = std::fs::canonicalize(parent).map_err(|_| cleanup_invalid())?;
    if canonical_parent != parent {
        return Err(cleanup_invalid());
    }
    crate::fs::validate_owned_dir(&canonical_parent)?;
    let target_name = target
        .file_name()
        .and_then(|value| value.to_str())
        .filter(|value| valid_target_name(value))
        .ok_or_else(cleanup_invalid)?
        .to_owned();
    Ok((canonical_parent, target_name))
}

fn acquire_restore_lock(parent: &Path, target_name: &str) -> Result<File, PlatformError> {
    let path = parent.join(format!(".{target_name}.restore.lock"));
    let lock = File::from(
        rustix::fs::open(
            &path,
            OFlags::RDWR | OFlags::CREATE | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::RUSR | Mode::WUSR,
        )
        .map_err(|_| cleanup_invalid())?,
    );
    crate::fs::validate_authority_fd(&lock)?;
    flock(&lock, FlockOperation::NonBlockingLockExclusive).map_err(|_| {
        PlatformError::new(
            ErrorCode::DataDirInUse,
            "restore target is owned by another offline command",
        )
    })?;
    Ok(lock)
}

fn canonical_uuid_v7(value: &str) -> bool {
    Uuid::parse_str(value)
        .ok()
        .is_some_and(|id| id.get_version_num() == 7 && id.hyphenated().to_string() == value)
}

fn valid_target_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && !matches!(value, "." | "..")
        && !value.bytes().any(|byte| byte.is_ascii_control())
}

fn cleanup_invalid() -> PlatformError {
    PlatformError::new(
        ErrorCode::RestoreInvalid,
        "failed restore staging cleanup failed validation",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt as _;

    #[test]
    fn cleanup_removes_only_one_exact_validated_restore_tree() {
        let temp = tempfile::tempdir().unwrap();
        let parent = fs::canonicalize(temp.path()).unwrap();
        let target = parent.join("data");
        let id = Uuid::now_v7().hyphenated().to_string();
        let staging = parent.join(format!(".data.restore-{id}"));
        fs::create_dir(&staging).unwrap();
        fs::set_permissions(&staging, fs::Permissions::from_mode(0o700)).unwrap();
        fs::write(staging.join("control.sqlite"), b"owned").unwrap();
        fs::set_permissions(
            staging.join("control.sqlite"),
            fs::Permissions::from_mode(0o600),
        )
        .unwrap();
        let unrelated = parent.join("unrelated");
        fs::create_dir(&unrelated).unwrap();
        fs::write(unrelated.join("keep"), b"preserved").unwrap();
        record_restore_failure(&target, &parent, &id).unwrap();

        let result = cleanup_restore_staging(&target, &id, 10, 100, 100).unwrap();
        assert_eq!(result.files, 1);
        assert_eq!(result.bytes, 5);
        assert!(!staging.exists());
        assert_eq!(fs::read(unrelated.join("keep")).unwrap(), b"preserved");
    }

    #[test]
    fn cleanup_refuses_symlinks_and_preserves_the_tree() {
        let temp = tempfile::tempdir().unwrap();
        let parent = fs::canonicalize(temp.path()).unwrap();
        let target = parent.join("data");
        let id = Uuid::now_v7().hyphenated().to_string();
        let staging = parent.join(format!(".data.restore-{id}"));
        fs::create_dir(&staging).unwrap();
        fs::set_permissions(&staging, fs::Permissions::from_mode(0o700)).unwrap();
        std::os::unix::fs::symlink(&parent, staging.join("control.sqlite")).unwrap();
        assert_eq!(
            cleanup_restore_staging(&target, &id, 10, 100, 100)
                .unwrap_err()
                .code(),
            ErrorCode::RestoreInvalid
        );
        assert!(staging.exists());
    }
}
