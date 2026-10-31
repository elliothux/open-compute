//! Path validation and same-filesystem atomic file replacement.

use open_compute_core::{ErrorCode, PlatformError};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::Path;
use uuid::Uuid;

const FILE_MODE: u32 = 0o600;
const DIR_MODE: u32 = 0o700;

/// Require `path` to be an already-validated absolute path without `..`.
pub(crate) fn require_absolute(path: &Path) -> Result<(), PlatformError> {
    if !path.is_absolute() {
        return Err(PlatformError::new(
            ErrorCode::PathInvalid,
            "storage path must be an absolute path",
        ));
    }
    if path
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err(PlatformError::new(
            ErrorCode::PathInvalid,
            "storage path must not contain '..'",
        ));
    }
    Ok(())
}

pub(crate) fn create_dir_secure(path: &Path) -> Result<(), PlatformError> {
    match fs::DirBuilder::new().mode(DIR_MODE).create(path) {
        Ok(()) => chmod(path, DIR_MODE)?,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(_) => {
            return Err(PlatformError::new(
                ErrorCode::PathInvalid,
                "failed to create owned directory",
            ));
        }
    }
    validate_owned_dir(path)
}

pub(crate) fn ensure_file_secure(path: &Path) -> Result<(), PlatformError> {
    if path.exists() {
        return validate_owned_file(path);
    }
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(FILE_MODE)
        .open(path)
        .map_err(|_| PlatformError::new(ErrorCode::PathInvalid, "failed to create owned file"))?;
    drop(file);
    chmod(path, FILE_MODE)?;
    validate_owned_file(path)
}

/// Verify an existing data root without creating or modifying it.
pub fn validate_root(path: &Path) -> Result<(), PlatformError> {
    require_absolute(path)?;
    let meta = fs::metadata(path).map_err(|_| {
        PlatformError::new(
            ErrorCode::PathInvalid,
            "data directory root is not accessible",
        )
    })?;
    if !meta.file_type().is_dir() {
        return Err(PlatformError::new(
            ErrorCode::PathInvalid,
            "data directory root must be a directory",
        ));
    }
    Ok(())
}

pub(crate) fn create_root_first_run(path: &Path) -> Result<(), PlatformError> {
    require_absolute(path)?;
    let parent = path.parent().ok_or_else(|| {
        PlatformError::new(
            ErrorCode::PathInvalid,
            "data directory root must have a parent",
        )
    })?;
    if !parent.is_dir() {
        return Err(PlatformError::new(
            ErrorCode::PathInvalid,
            "data directory parent must already exist",
        ));
    }
    fs::create_dir(path).map_err(|_| {
        PlatformError::new(
            ErrorCode::PathInvalid,
            "failed to create data directory root",
        )
    })?;
    chmod(path, DIR_MODE)?;
    validate_root(path)
}

pub(crate) fn validate_owned_dir(path: &Path) -> Result<(), PlatformError> {
    let meta = inspect(path)?;
    if !meta.file_type().is_dir() {
        return Err(PlatformError::new(
            ErrorCode::PathInvalid,
            "owned path must be a directory",
        ));
    }
    Ok(())
}

/// Require a regular file at the selected path.
pub fn validate_owned_file(path: &Path) -> Result<(), PlatformError> {
    let meta = inspect(path)?;
    if !meta.file_type().is_file() {
        return Err(PlatformError::new(
            ErrorCode::PathInvalid,
            "owned path must be a regular file",
        ));
    }
    Ok(())
}

pub(crate) fn inspect(path: &Path) -> Result<fs::Metadata, PlatformError> {
    fs::metadata(path)
        .map_err(|_| PlatformError::new(ErrorCode::PathInvalid, "owned path is not accessible"))
}

/// Open `path`; newly created files use mode 0600.
pub(crate) fn open_file(path: &Path, create: bool, write: bool) -> Result<File, PlatformError> {
    require_absolute(path)?;
    let mut flags = rustix::fs::OFlags::CLOEXEC;
    flags |= if write {
        rustix::fs::OFlags::RDWR
    } else {
        rustix::fs::OFlags::RDONLY
    };
    if create {
        flags |= rustix::fs::OFlags::CREATE;
    }
    let mode = if create {
        rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR
    } else {
        rustix::fs::Mode::empty()
    };
    let fd = rustix::fs::open(path, flags, mode).map_err(|err| {
        if err == rustix::io::Errno::LOOP {
            PlatformError::new(ErrorCode::PathInvalid, "owned path must not be a symlink")
        } else {
            PlatformError::new(
                ErrorCode::PathInvalid,
                "failed to open path without following",
            )
        }
    })?;
    Ok(File::from(fd))
}

/// Require an already-opened regular file.
pub(crate) fn validate_authority_fd(file: &File) -> Result<(), PlatformError> {
    let meta = file.metadata().map_err(|_| {
        PlatformError::new(
            ErrorCode::PathInvalid,
            "failed to fstat opened authority file",
        )
    })?;
    if !meta.file_type().is_file() {
        return Err(PlatformError::new(
            ErrorCode::PathInvalid,
            "owned path must be a regular file",
        ));
    }
    Ok(())
}

pub(crate) fn fsync_dir(path: &Path) -> Result<(), PlatformError> {
    let dir = File::open(path).map_err(|_| {
        PlatformError::new(ErrorCode::PathInvalid, "failed to open parent directory")
    })?;
    dir.sync_all()
        .map_err(|_| PlatformError::new(ErrorCode::PathInvalid, "failed to fsync parent directory"))
}

pub(crate) fn chmod(path: &Path, mode: u32) -> Result<(), PlatformError> {
    let mut perms = fs::metadata(path)
        .map_err(|_| PlatformError::new(ErrorCode::PathInvalid, "failed to read permissions"))?
        .permissions();
    perms.set_mode(mode);
    fs::set_permissions(path, perms)
        .map_err(|_| PlatformError::new(ErrorCode::PathInvalid, "failed to set secure permissions"))
}

/// Ensure `child` is a canonical descendant of `root` and a regular file or directory.
pub(crate) fn validate_contained(root: &Path, child: &Path) -> Result<(), PlatformError> {
    require_absolute(root)?;
    require_absolute(child)?;
    if !child.starts_with(root) {
        return Err(PlatformError::new(
            ErrorCode::PathInvalid,
            "owned path escapes the data directory",
        ));
    }
    Ok(())
}

/// Write `contents` to `path` via a unique temp file on the same filesystem.
pub fn atomic_write(path: &Path, contents: &[u8]) -> Result<(), PlatformError> {
    require_absolute(path)?;
    let parent = path.parent().ok_or_else(|| {
        PlatformError::new(
            ErrorCode::PathInvalid,
            "atomic write path must have a parent",
        )
    })?;
    validate_owned_dir(parent)?;
    let temp_name = format!(".tmp-{}", Uuid::now_v7().as_hyphenated());
    let temp_path = parent.join(temp_name);
    let result = write_temp_then_rename(parent, path, &temp_path, contents);
    if result.is_err() {
        let _ = fs::remove_file(&temp_path);
    }
    result
}

/// Create `path` as a mode `0700` directory, or validate an existing owned directory.
pub fn ensure_dir_secure(path: &Path) -> Result<(), PlatformError> {
    require_absolute(path)?;
    create_dir_secure(path)
}

fn write_temp_then_rename(
    parent: &Path,
    final_path: &Path,
    temp_path: &Path,
    contents: &[u8],
) -> Result<(), PlatformError> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(FILE_MODE)
        .open(temp_path)
        .map_err(|_| PlatformError::new(ErrorCode::PathInvalid, "failed to create temp file"))?;
    file.write_all(contents).map_err(|_| {
        PlatformError::new(ErrorCode::PathInvalid, "failed to write atomic temp file")
    })?;
    file.sync_all().map_err(|_| {
        PlatformError::new(ErrorCode::PathInvalid, "failed to fsync atomic temp file")
    })?;
    drop(file);
    fs::rename(temp_path, final_path).map_err(|_| {
        PlatformError::new(
            ErrorCode::PathInvalid,
            "failed to rename atomic temp file into place",
        )
    })?;
    fsync_dir(parent)?;
    Ok(())
}
