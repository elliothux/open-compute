//! Host-path I/O, bounded asset reads, and atomic publication.

use open_compute_core::{ErrorCode, PlatformError};
use rustix::fd::{AsFd, OwnedFd};
use rustix::fs::{
    AtFlags, Mode, OFlags, RenameFlags, fchmod, fstat, fsync, mkdirat, open, openat, renameat,
    renameat_with, statat, unlinkat,
};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::path::{Component, Path, PathBuf};

pub(crate) const FILE_MODE: u32 = 0o600;
pub(crate) const MAX_LOCK_BYTES: u64 = 1024 * 1024;
pub(crate) const MAX_ASSET_FILE_BYTES: u64 = 1024 * 1024;
pub(crate) const MAX_ASSETS_TOTAL_BYTES: u64 = 8 * 1024 * 1024;
pub(crate) const MAX_ASSET_FILES: usize = 4096;
pub(crate) const MAX_ASSET_ENTRIES: usize = 8192;
pub(crate) const MAX_WALK_DEPTH: usize = 8;

fn path_invalid(msg: &'static str) -> PlatformError {
    PlatformError::new(ErrorCode::PathInvalid, msg)
}

pub(crate) fn require_absolute(path: &Path) -> Result<(), PlatformError> {
    if !path.is_absolute() {
        return Err(path_invalid("path must be an absolute path"));
    }
    if path.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err(path_invalid("path must not contain '..'"));
    }
    Ok(())
}

pub(crate) fn open_dir(path: &Path) -> Result<OwnedFd, PlatformError> {
    require_absolute(path)?;
    open(
        path,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|_| path_invalid("failed to open runtime directory"))
}

/// Open an existing absolute host directory.
pub fn open_host_directory(path: &Path) -> Result<OwnedFd, PlatformError> {
    open_dir(path)
}

fn open_parent_dir(path: &Path) -> Result<(OwnedFd, std::ffi::OsString), PlatformError> {
    let parent = path
        .parent()
        .ok_or_else(|| path_invalid("destination must have a parent"))?;
    let name = path
        .file_name()
        .ok_or_else(|| path_invalid("destination must have a file name"))?
        .to_os_string();
    Ok((open_dir(parent)?, name))
}

/// Open the selected host path; exclusive creation uses mode 0600.
pub(crate) fn open_file(path: &Path, write: bool, create: bool) -> Result<File, PlatformError> {
    let (parent, name) = open_parent_dir(path)?;
    let mut flags = OFlags::CLOEXEC;
    flags |= if write { OFlags::RDWR } else { OFlags::RDONLY };
    if create {
        flags |= OFlags::CREATE | OFlags::EXCL;
    }
    let mode = if create {
        Mode::RUSR | Mode::WUSR
    } else {
        Mode::empty()
    };
    let fd = openat(&parent, name.as_os_str(), flags, mode)
        .map_err(|_| path_invalid("failed to open path"))?;
    Ok(File::from(fd))
}

/// Open an optional existing host path.
///
/// Only a missing final component is reported as `None`; unsafe or inaccessible
/// paths remain errors so callers cannot confuse a failed read with absence.
pub(crate) fn open_optional_file(path: &Path) -> Result<Option<File>, PlatformError> {
    let (parent, name) = open_parent_dir(path)?;
    match openat(
        &parent,
        name.as_os_str(),
        OFlags::CLOEXEC | OFlags::RDONLY,
        Mode::empty(),
    ) {
        Ok(fd) => Ok(Some(File::from(fd))),
        Err(err) if err == rustix::io::Errno::NOENT => Ok(None),
        Err(_) => Err(path_invalid("failed to open path")),
    }
}

fn fstatat_nofollow(
    parent: &OwnedFd,
    name: &std::ffi::OsStr,
) -> Result<rustix::fs::Stat, PlatformError> {
    statat(parent, name, AtFlags::SYMLINK_NOFOLLOW)
        .map_err(|_| path_invalid("path is not accessible"))
}

pub(crate) fn require_regular_file(path: &Path) -> Result<fs::Metadata, PlatformError> {
    let file = open_file(path, false, false)?;
    let meta = file
        .metadata()
        .map_err(|_| path_invalid("path is not accessible"))?;
    if !meta.file_type().is_file() {
        return Err(path_invalid("path must be a regular file"));
    }
    Ok(meta)
}

pub(crate) fn hex_sha256(digest: &[u8; 32]) -> String {
    hex::encode(digest)
}

pub(crate) fn parse_sha256_hex(value: &str) -> Result<[u8; 32], PlatformError> {
    if value.len() != 64 || !value.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(PlatformError::new(
            ErrorCode::RuntimeInvalid,
            "sha256 digest must be 64 lowercase hex characters",
        ));
    }
    if value.bytes().any(|b| b.is_ascii_uppercase()) {
        return Err(PlatformError::new(
            ErrorCode::RuntimeInvalid,
            "sha256 digest must be 64 lowercase hex characters",
        ));
    }
    let mut out = [0u8; 32];
    hex::decode_to_slice(value, &mut out).map_err(|_| {
        PlatformError::new(
            ErrorCode::RuntimeInvalid,
            "sha256 digest must be 64 lowercase hex characters",
        )
    })?;
    Ok(out)
}

fn fchmod_file(file: &File, mode: u32) -> Result<(), PlatformError> {
    let rustix_mode = Mode::from_bits_truncate(mode as _);
    fchmod(file, rustix_mode).map_err(|_| path_invalid("failed to set permissions"))
}

pub(crate) fn chmod(path: &Path, mode: u32) -> Result<(), PlatformError> {
    if let Ok(file) = open_file(path, true, false) {
        return fchmod_file(&file, mode);
    }
    let dir = open_dir(path)?;
    fchmod(&dir, Mode::from_bits_truncate(mode as _))
        .map_err(|_| path_invalid("failed to set permissions"))
}

pub(crate) fn create_dir_secure(path: &Path) -> Result<(), PlatformError> {
    require_absolute(path)?;
    use std::os::unix::fs::DirBuilderExt;
    match fs::DirBuilder::new().mode(0o700).create(path) {
        Ok(()) => fs::set_permissions(path, fs::Permissions::from_mode(0o700))
            .map_err(|_| path_invalid("failed to set runtime directory permissions")),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists && path.is_dir() => Ok(()),
        Err(_) => Err(path_invalid("failed to create runtime data directory")),
    }
}

pub(crate) fn fsync_dir(path: &Path) -> Result<(), PlatformError> {
    let dir = open_dir(path)?;
    fsync(dir.as_fd()).map_err(|_| path_invalid("failed to fsync parent directory"))
}

/// Atomically replace `dest` with `contents` (0600-capable). Does not unlink `dest` first.
pub(crate) fn write_atomic_replace(
    dest: &Path,
    contents: &[u8],
    mode: u32,
) -> Result<(), PlatformError> {
    require_absolute(dest)?;
    let (parent_fd, dest_name) = open_parent_dir(dest)?;
    let tmp_name = format!(".partial.{}", uuid::Uuid::now_v7());
    let mut tmp_guard = TmpGuard {
        parent: parent_fd,
        name: tmp_name.clone(),
        persist: false,
    };
    {
        let flags = OFlags::CLOEXEC | OFlags::RDWR | OFlags::CREATE | OFlags::EXCL;
        let fd = openat(
            &tmp_guard.parent,
            tmp_name.as_str(),
            flags,
            Mode::RUSR | Mode::WUSR,
        )
        .map_err(|_| path_invalid("failed to write temporary file"))?;
        let mut file = File::from(fd);
        file.write_all(contents)
            .map_err(|_| path_invalid("failed to write temporary file"))?;
        file.sync_all()
            .map_err(|_| path_invalid("failed to fsync temporary file"))?;
        fchmod_file(&file, mode)?;
    }
    renameat(
        &tmp_guard.parent,
        tmp_name.as_str(),
        &tmp_guard.parent,
        dest_name.as_os_str(),
    )
    .map_err(|_| path_invalid("failed to publish atomically"))?;
    tmp_guard.persist = true;
    fsync_dir(dest.parent().unwrap_or(dest))?;
    Ok(())
}

/// Atomically publish `old` to `new` and fail if `new` already exists.
pub(crate) fn rename_noreplace(old: &Path, new: &Path) -> Result<(), PlatformError> {
    run_publish_hook(new);
    let (old_parent, old_name) = open_parent_dir(old)?;
    let (new_parent, new_name) = open_parent_dir(new)?;
    renameat_with(
        &old_parent,
        old_name.as_os_str(),
        &new_parent,
        new_name.as_os_str(),
        RenameFlags::NOREPLACE,
    )
    .map_err(|err| {
        if err == rustix::io::Errno::EXIST || err == rustix::io::Errno::NOTEMPTY {
            path_invalid("refusing to overwrite an existing path")
        } else {
            path_invalid("failed to publish atomically")
        }
    })
}

pub(crate) fn write_atomic_new(
    dest: &Path,
    contents: &[u8],
    mode: u32,
) -> Result<(), PlatformError> {
    require_absolute(dest)?;
    let (parent_fd, dest_name) = open_parent_dir(dest)?;
    if fstatat_nofollow(&parent_fd, dest_name.as_os_str()).is_ok() {
        return Err(path_invalid("refusing to overwrite an existing path"));
    }
    let tmp_name = format!(".partial.{}", uuid::Uuid::now_v7());
    let tmp_path = dest
        .parent()
        .ok_or_else(|| path_invalid("destination must have a parent"))?
        .join(&tmp_name);
    let mut tmp_guard = TmpGuard {
        parent: parent_fd,
        name: tmp_name.clone(),
        persist: false,
    };
    {
        let flags = OFlags::CLOEXEC | OFlags::RDWR | OFlags::CREATE | OFlags::EXCL;
        let fd = openat(
            &tmp_guard.parent,
            tmp_name.as_str(),
            flags,
            Mode::RUSR | Mode::WUSR,
        )
        .map_err(|_| path_invalid("failed to write temporary file"))?;
        let mut file = File::from(fd);
        file.write_all(contents)
            .map_err(|_| path_invalid("failed to write temporary file"))?;
        file.sync_all()
            .map_err(|_| path_invalid("failed to fsync temporary file"))?;
        fchmod_file(&file, mode)?;
    }
    match rename_noreplace(&tmp_path, dest) {
        Ok(()) => {
            tmp_guard.persist = true;
            fsync_dir(dest.parent().unwrap_or(dest))?;
            Ok(())
        }
        Err(err) => Err(err),
    }
}

struct TmpGuard {
    parent: OwnedFd,
    name: String,
    persist: bool,
}

impl Drop for TmpGuard {
    fn drop(&mut self) {
        if !self.persist {
            let _ = unlinkat(&self.parent, self.name.as_str(), AtFlags::empty());
        }
    }
}

pub(crate) fn contained_in(root: &Path, child: &Path) -> Result<(), PlatformError> {
    require_absolute(root)?;
    require_absolute(child)?;
    let root_comps: Vec<_> = root.components().collect();
    let child_comps: Vec<_> = child.components().collect();
    if child_comps.len() < root_comps.len() {
        return Err(path_invalid(
            "path must be contained in the trusted assets directory",
        ));
    }
    if root_comps != child_comps[..root_comps.len()] {
        return Err(path_invalid(
            "path must be contained in the trusted assets directory",
        ));
    }
    Ok(())
}

pub(crate) fn read_regular_file_bounded(
    path: &Path,
    max_bytes: u64,
) -> Result<Vec<u8>, PlatformError> {
    let mut file = open_file(path, false, false)?;
    let meta = file
        .metadata()
        .map_err(|_| path_invalid("failed to stat opened file"))?;
    if !meta.file_type().is_file() {
        return Err(path_invalid("path must be a regular file"));
    }
    if meta.len() > max_bytes {
        return Err(path_invalid("file exceeds the configured size bound"));
    }
    let mut buf = Vec::new();
    file.read_to_end(&mut buf)
        .map_err(|_| path_invalid("failed to read regular file"))?;
    if buf.len() as u64 > max_bytes {
        return Err(path_invalid("file exceeds the configured size bound"));
    }
    Ok(buf)
}

pub(crate) fn read_regular_file(path: &Path) -> Result<Vec<u8>, PlatformError> {
    read_regular_file_bounded(path, MAX_ASSET_FILE_BYTES)
}

pub(crate) fn list_files_sorted(dir: &Path) -> Result<Vec<PathBuf>, PlatformError> {
    let mut files = 0usize;
    let mut entries = 0usize;
    list_files_sorted_inner(dir, 0, &mut files, &mut entries)
}

fn max_asset_files() -> usize {
    #[cfg(test)]
    {
        let override_n = TEST_MAX_ASSET_FILES
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(n) = *override_n {
            return n;
        }
    }
    MAX_ASSET_FILES
}

fn max_asset_entries() -> usize {
    #[cfg(test)]
    {
        let override_n = TEST_MAX_ASSET_ENTRIES
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(n) = *override_n {
            return n;
        }
    }
    MAX_ASSET_ENTRIES
}

fn list_files_sorted_inner(
    dir: &Path,
    depth: usize,
    files: &mut usize,
    entries: &mut usize,
) -> Result<Vec<PathBuf>, PlatformError> {
    if depth > MAX_WALK_DEPTH {
        return Err(path_invalid(
            "assets directory exceeds the maximum walk depth",
        ));
    }
    let dir_fd = open_dir(dir)?;
    let mut names = Vec::new();
    let rustix_dir = rustix::fs::Dir::read_from(&dir_fd)
        .map_err(|_| path_invalid("failed to read assets directory"))?;
    let mut dirents = Vec::new();
    for entry in rustix_dir {
        let entry = entry.map_err(|_| path_invalid("failed to read assets directory entry"))?;
        let raw = entry.file_name();
        let bytes = raw.to_bytes();
        if bytes == b"." || bytes == b".." {
            continue;
        }
        *entries = entries.saturating_add(1);
        if *entries > max_asset_entries() {
            return Err(path_invalid(
                "assets directory exceeds the maximum entry count",
            ));
        }
        dirents.push(std::ffi::OsStr::from_bytes(bytes).to_os_string());
    }
    dirents.sort();
    for name in dirents {
        let path = dir.join(&name);
        let child = openat(
            &dir_fd,
            name.as_os_str(),
            OFlags::RDONLY | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|_| path_invalid("failed to read assets directory entry"))?;
        let stat = fstat(&child).map_err(|_| path_invalid("failed to stat opened file"))?;
        let ftype = rustix::fs::FileType::from_raw_mode(stat.st_mode);
        if ftype == rustix::fs::FileType::Directory {
            names.extend(list_files_sorted_inner(&path, depth + 1, files, entries)?);
        } else if ftype == rustix::fs::FileType::RegularFile {
            *files = files.saturating_add(1);
            if *files > max_asset_files() {
                return Err(path_invalid(
                    "assets directory exceeds the maximum file count",
                ));
            }
            if stat.st_size as u64 > MAX_ASSET_FILE_BYTES {
                return Err(path_invalid("file exceeds the configured size bound"));
            }
            names.push(path);
        } else {
            return Err(path_invalid("path must not be a special file"));
        }
    }
    names.sort();
    Ok(names)
}

pub(crate) fn remove_file_strict(path: &Path) -> Result<(), PlatformError> {
    let Ok((parent, name)) = open_parent_dir(path) else {
        return Ok(());
    };
    match unlinkat(&parent, name.as_os_str(), AtFlags::empty()) {
        Ok(()) => Ok(()),
        Err(err) if err == rustix::io::Errno::NOENT => Ok(()),
        Err(_) => Err(PlatformError::new(
            ErrorCode::CacheEntryCorrupt,
            "failed to remove a corrupt cache entry",
        )),
    }
}

pub(crate) fn remove_file_nofollow(path: &Path) -> Result<(), PlatformError> {
    let (parent, name) = open_parent_dir(path)?;
    match unlinkat(&parent, name.as_os_str(), AtFlags::empty()) {
        Ok(()) => Ok(()),
        Err(err) if err == rustix::io::Errno::NOENT => Ok(()),
        Err(_) => Err(path_invalid("failed to remove runtime staging file")),
    }
}

#[cfg(any(test, target_os = "macos"))]
pub(crate) fn remove_empty_dir_nofollow(path: &Path) -> Result<(), PlatformError> {
    let (parent, name) = open_parent_dir(path)?;
    match unlinkat(&parent, name.as_os_str(), AtFlags::REMOVEDIR) {
        Ok(()) => Ok(()),
        Err(err) if err == rustix::io::Errno::NOENT => Ok(()),
        Err(_) => Err(path_invalid("failed to remove runtime staging directory")),
    }
}

/// Same-directory workspace that is removed on drop unless published.
mod workdir;

pub(crate) use workdir::{StagingDir, WorkDir};

#[cfg(test)]
use std::sync::{Arc, Mutex};

#[cfg(test)]
static TEST_MAX_ASSET_FILES: Mutex<Option<usize>> = Mutex::new(None);

#[cfg(test)]
static TEST_MAX_ASSET_ENTRIES: Mutex<Option<usize>> = Mutex::new(None);

#[cfg(test)]
pub(crate) fn set_test_max_asset_files(n: Option<usize>) {
    *TEST_MAX_ASSET_FILES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = n;
}

#[cfg(test)]
pub(crate) fn set_test_max_asset_entries(n: Option<usize>) {
    *TEST_MAX_ASSET_ENTRIES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = n;
}

#[cfg(test)]
type PublishHook = Arc<dyn Fn(&Path) + Send + Sync>;

#[cfg(test)]
static PUBLISH_HOOK: Mutex<Option<PublishHook>> = Mutex::new(None);

#[cfg(test)]
pub(crate) fn set_publish_hook(hook: impl Fn(&Path) + Send + Sync + 'static) {
    *PUBLISH_HOOK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Arc::new(hook));
}

#[cfg(test)]
pub(crate) fn clear_publish_hook() {
    *PUBLISH_HOOK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
}

fn run_publish_hook(dest: &Path) {
    #[cfg(test)]
    {
        if let Some(hook) = PUBLISH_HOOK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
        {
            hook(dest);
        }
    }
    #[cfg(not(test))]
    {
        let _ = dest;
    }
}

#[cfg(test)]
#[path = "fsutil_tests.rs"]
mod coverage_tests;
