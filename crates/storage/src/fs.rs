//! Path validation and same-filesystem atomic file replacement.

use open_compute_core::{ErrorCode, PlatformError};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::Path;
use uuid::Uuid;

const FILE_MODE: u32 = 0o600;
const DIR_MODE: u32 = 0o700;
const GROUP_WORLD_WRITE: u32 = 0o022;

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
    if path.exists() {
        return validate_owned_dir(path);
    }
    fs::create_dir(path).map_err(|_| {
        PlatformError::new(ErrorCode::PathInvalid, "failed to create owned directory")
    })?;
    chmod(path, DIR_MODE)?;
    validate_owned_dir(path)
}

pub(crate) fn ensure_file_secure(path: &Path) -> Result<(), PlatformError> {
    if path.exists() {
        return validate_owned_file(path, true);
    }
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(FILE_MODE)
        .open(path)
        .map_err(|_| PlatformError::new(ErrorCode::PathInvalid, "failed to create owned file"))?;
    drop(file);
    chmod(path, FILE_MODE)?;
    validate_owned_file(path, true)
}

pub(crate) fn validate_root(path: &Path) -> Result<(), PlatformError> {
    require_absolute(path)?;
    let meta = fs::symlink_metadata(path).map_err(|_| {
        PlatformError::new(
            ErrorCode::PathInvalid,
            "data directory root is not accessible",
        )
    })?;
    if meta.file_type().is_symlink() {
        return Err(PlatformError::new(
            ErrorCode::PathInvalid,
            "data directory root must not be a symlink",
        ));
    }
    if !meta.file_type().is_dir() {
        return Err(PlatformError::new(
            ErrorCode::PathInvalid,
            "data directory root must be a directory",
        ));
    }
    reject_group_world_writable(&meta, true)?;
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
    if meta.file_type().is_symlink() {
        return Err(PlatformError::new(
            ErrorCode::PathInvalid,
            "owned path must not be a symlink",
        ));
    }
    if !meta.file_type().is_dir() {
        return Err(PlatformError::new(
            ErrorCode::PathInvalid,
            "owned path must be a directory",
        ));
    }
    reject_group_world_writable(&meta, true)?;
    Ok(())
}

pub(crate) fn validate_owned_file(path: &Path, authority: bool) -> Result<(), PlatformError> {
    let meta = inspect(path)?;
    if meta.file_type().is_symlink() {
        return Err(PlatformError::new(
            ErrorCode::PathInvalid,
            "owned path must not be a symlink",
        ));
    }
    if !meta.file_type().is_file() {
        return Err(PlatformError::new(
            ErrorCode::PathInvalid,
            "owned path must be a regular file",
        ));
    }
    reject_group_world_writable(&meta, false)?;
    if authority {
        let mode = meta.permissions().mode() & 0o777;
        if mode != FILE_MODE {
            return Err(PlatformError::new(
                ErrorCode::PathInvalid,
                "authority file must have mode 0600",
            ));
        }
    }
    Ok(())
}

pub(crate) fn inspect(path: &Path) -> Result<fs::Metadata, PlatformError> {
    fs::symlink_metadata(path)
        .map_err(|_| PlatformError::new(ErrorCode::PathInvalid, "owned path is not accessible"))
}

fn reject_group_world_writable(meta: &fs::Metadata, _dir: bool) -> Result<(), PlatformError> {
    let mode = meta.permissions().mode() & 0o777;
    if mode & GROUP_WORLD_WRITE != 0 {
        return Err(PlatformError::new(
            ErrorCode::PathInvalid,
            "owned path must not be group or world writable",
        ));
    }
    Ok(())
}

/// Open `path` with `O_NOFOLLOW`. `create` uses `O_CREAT` with mode 0600 and does not chmod an existing file.
pub(crate) fn open_nofollow(path: &Path, create: bool, write: bool) -> Result<File, PlatformError> {
    require_absolute(path)?;
    let mut flags = rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::CLOEXEC;
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

/// Validate an already-opened fd is a regular file with exact mode 0600.
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
    let mode = meta.permissions().mode() & 0o777;
    if mode != FILE_MODE {
        return Err(PlatformError::new(
            ErrorCode::PathInvalid,
            "authority file must have mode 0600",
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
    if child.exists() || fs::symlink_metadata(child).is_ok() {
        let meta = inspect(child)?;
        if meta.file_type().is_symlink() {
            return Err(PlatformError::new(
                ErrorCode::PathInvalid,
                "owned path must not be a symlink",
            ));
        }
        let ft = meta.file_type();
        if !(ft.is_file() || ft.is_dir()) {
            return Err(PlatformError::new(
                ErrorCode::PathInvalid,
                "owned path must not be a special file",
            ));
        }
    }
    let root_canon = fs::canonicalize(root).map_err(|_| {
        PlatformError::new(
            ErrorCode::PathInvalid,
            "data directory root cannot be canonicalized",
        )
    })?;
    let check = if child.exists() {
        fs::canonicalize(child).map_err(|_| {
            PlatformError::new(ErrorCode::PathInvalid, "owned path cannot be canonicalized")
        })?
    } else {
        let parent = child.parent().ok_or_else(|| {
            PlatformError::new(ErrorCode::PathInvalid, "owned path must have a parent")
        })?;
        let parent_canon = fs::canonicalize(parent).map_err(|_| {
            PlatformError::new(
                ErrorCode::PathInvalid,
                "owned path parent cannot be canonicalized",
            )
        })?;
        parent_canon.join(child.file_name().ok_or_else(|| {
            PlatformError::new(ErrorCode::PathInvalid, "owned path must have a file name")
        })?)
    };
    if !check.starts_with(&root_canon) {
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

fn write_temp_then_rename(
    parent: &Path,
    final_path: &Path,
    temp_path: &Path,
    contents: &[u8],
) -> Result<(), PlatformError> {
    let parent_meta = inspect(parent)?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(FILE_MODE)
        .open(temp_path)
        .map_err(|_| PlatformError::new(ErrorCode::PathInvalid, "failed to create temp file"))?;
    let temp_meta = inspect(temp_path)?;
    if temp_meta.dev() != parent_meta.dev() {
        return Err(PlatformError::new(
            ErrorCode::PathInvalid,
            "temp file is not on the same filesystem as the destination",
        ));
    }
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
    if final_path.exists() {
        validate_owned_file(final_path, true)?;
    }
    Ok(())
}
