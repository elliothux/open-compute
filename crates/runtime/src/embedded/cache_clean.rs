//! Explicit cleanup of inactive, materialized embedded runtime packages.

use super::payload;
use crate::fsutil::{open_dir_nofollow, open_nofollow};
use open_compute_core::{ErrorCode, PlatformError};
use rustix::fs::{AtFlags, Dir, FileType, Mode, OFlags, openat, statat};
use std::fs;
use std::path::Path;

const MAX_ENTRIES: usize = 8192;

/// Per-scope result of cleaning known embedded runtime packages.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RuntimeCacheCleanReport {
    /// Physical bytes removed, or eligible bytes during a dry run.
    pub bytes: u64,
    /// Packages removed, or eligible packages during a dry run.
    pub entries: u64,
    /// Pinned or unrecognized package-directory entries retained.
    pub skipped: u64,
    /// Recognized packages whose removal failed.
    pub failed: u64,
}

/// Clean inactive package copies below an existing private cache root.
/// The caller must own the OCD scope lock and finish orphan recovery first.
pub fn clean_embedded_runtime_cache(
    cache_root: &Path,
    dry_run: bool,
) -> Result<RuntimeCacheCleanReport, PlatformError> {
    if !cache_root.is_absolute() {
        return Err(invalid("runtime cache root must be absolute"));
    }
    match fs::symlink_metadata(cache_root) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(RuntimeCacheCleanReport::default());
        }
        Err(_) => return Err(invalid("runtime cache root is inaccessible")),
        Ok(_) => {
            let _ = open_dir_nofollow(cache_root)?;
        }
    }
    let packages = cache_root.join("packages");
    match fs::symlink_metadata(&packages) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(RuntimeCacheCleanReport::default());
        }
        Err(_) => return Err(invalid("runtime package directory is inaccessible")),
        Ok(_) => {}
    }
    let directory = open_dir_nofollow(&packages)?;
    let mut report = RuntimeCacheCleanReport::default();
    let entries = Dir::read_from(&directory)
        .map_err(|_| invalid("failed to read runtime package directory"))?;
    for (count, entry) in entries.enumerate() {
        if count >= MAX_ENTRIES {
            return Err(invalid("runtime package directory exceeds its entry bound"));
        }
        let entry = entry.map_err(|_| invalid("failed to read runtime package entry"))?;
        let Ok(name) = entry.file_name().to_str() else {
            report.skipped += 1;
            continue;
        };
        if name == "." || name == ".." {
            continue;
        }
        if name == payload::PAYLOAD_SHA256 || !is_digest(name) {
            report.skipped += 1;
            continue;
        }
        let path = packages.join(name);
        if open_dir_nofollow(&path).is_err()
            || !open_nofollow(&path.join("runtime/workerd.lock.json"), false, false)
                .is_ok_and(|file| file.metadata().is_ok_and(|meta| meta.is_file()))
        {
            report.skipped += 1;
            continue;
        }
        let Some(bytes) = package_bytes(&path)? else {
            report.skipped += 1;
            continue;
        };
        if !dry_run && fs::remove_dir_all(&path).is_err() {
            report.failed += 1;
            continue;
        }
        report.entries += 1;
        report.bytes = report.bytes.saturating_add(bytes);
    }
    Ok(report)
}

fn is_digest(name: &str) -> bool {
    name.len() == 64
        && name
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn package_bytes(root: &Path) -> Result<Option<u64>, PlatformError> {
    let mut directories = vec![open_dir_nofollow(root)?];
    let mut seen = 0;
    let mut bytes = 0_u64;
    while let Some(directory) = directories.pop() {
        let entries =
            Dir::read_from(&directory).map_err(|_| invalid("failed to inspect runtime package"))?;
        for entry in entries {
            seen += 1;
            if seen > MAX_ENTRIES {
                return Ok(None);
            }
            let entry = entry.map_err(|_| invalid("failed to inspect runtime package entry"))?;
            let name = entry.file_name();
            if name.to_bytes() == b"." || name.to_bytes() == b".." {
                continue;
            }
            let stat = statat(&directory, name, AtFlags::SYMLINK_NOFOLLOW)
                .map_err(|_| invalid("failed to inspect runtime package entry"))?;
            match FileType::from_raw_mode(stat.st_mode) {
                FileType::Directory => {
                    let child = openat(
                        &directory,
                        name,
                        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                        Mode::empty(),
                    )
                    .map_err(|_| invalid("failed to open runtime package directory"))?;
                    directories.push(child);
                }
                FileType::RegularFile => {
                    bytes = bytes.saturating_add(u64::try_from(stat.st_size).unwrap_or(0));
                }
                _ => return Ok(None),
            }
        }
    }
    Ok(Some(bytes))
}

fn invalid(message: &'static str) -> PlatformError {
    PlatformError::new(ErrorCode::PathInvalid, message)
}

#[cfg(test)]
#[path = "cache_clean_tests.rs"]
mod tests;
