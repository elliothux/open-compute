//! Data-directory advisory exclusive lock.

use crate::fs;
use open_compute_core::{ErrorCode, PlatformError, StartupId};
use rustix::fs::{FlockOperation, flock};
use serde::Serialize;
use std::fs::File;
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Best-effort classification of the backing filesystem.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FilesystemDurability {
    /// Local disk that appears suitable for advisory locks and fsync.
    ApparentlyLocal,
    /// Network or remote filesystem where lock/durability semantics are weaker.
    NetworkOrRemote,
    /// Classification failed; must not be treated as safe.
    Unclassified,
}

impl FilesystemDurability {
    /// Operator warning for `doctor`. [`Self::ApparentlyLocal`] is the only quiet value.
    #[must_use]
    pub fn doctor_warning(self) -> Option<&'static str> {
        match self {
            Self::ApparentlyLocal => None,
            Self::NetworkOrRemote => Some(
                "data directory appears to be on a network filesystem; advisory locks and fsync may not be durable",
            ),
            Self::Unclassified => Some(
                "data directory filesystem type could not be classified; lock and durability safety is not claimed",
            ),
        }
    }
}

#[derive(Serialize)]
struct LockMetadata {
    startup_id: String,
    platform_id: Option<String>,
    pid: u32,
    started_at_unix_ms: u64,
    release_version: String,
}

/// Held exclusive advisory lock on `platform.lock`.
#[derive(Debug)]
pub struct DataDirLock {
    path: PathBuf,
    file: File,
    startup_id: StartupId,
    durability: FilesystemDurability,
}

impl DataDirLock {
    pub(crate) fn acquire(path: &Path, startup_id: StartupId) -> Result<Self, PlatformError> {
        fs::require_absolute(path)?;
        let file = fs::open_nofollow(path, true, true).map_err(|_| {
            PlatformError::new(
                ErrorCode::PathInvalid,
                "failed to open data directory lock file",
            )
        })?;
        fs::validate_authority_fd(&file)?;
        flock(&file, FlockOperation::NonBlockingLockExclusive).map_err(|_| {
            PlatformError::new(
                ErrorCode::DataDirInUse,
                "data directory exclusive lock is held by another instance",
            )
        })?;
        let durability = classify_durability(path);
        let lock = Self {
            path: path.to_path_buf(),
            file,
            startup_id,
            durability,
        };
        lock.write_metadata(None)?;
        Ok(lock)
    }

    /// Probe exclusive lock availability without writing metadata or retaining the lock.
    pub fn probe_available(path: &Path) -> Result<bool, PlatformError> {
        Ok(InspectLock::try_acquire(path)?.is_some())
    }

    /// Classify backing filesystem durability without taking the lock.
    #[must_use]
    pub fn classify_path(path: &Path) -> FilesystemDurability {
        classify_durability(path)
    }

    /// Startup generation recorded in the lock diagnostic file.
    #[must_use]
    pub fn startup_id(&self) -> StartupId {
        self.startup_id
    }

    /// Best-effort durability classification for doctor.
    #[must_use]
    pub fn filesystem_durability(&self) -> FilesystemDurability {
        self.durability
    }

    /// Path of the lock file. Ownership is the advisory lock, not this path string.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn write_metadata(&self, platform_id: Option<&str>) -> Result<(), PlatformError> {
        let started_at_unix_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as u64);
        let meta = LockMetadata {
            startup_id: self.startup_id.to_string(),
            platform_id: platform_id.map(str::to_string),
            pid: std::process::id(),
            started_at_unix_ms,
            release_version: env!("CARGO_PKG_VERSION").to_string(),
        };
        let json = serde_json::to_vec(&meta).map_err(|_| {
            PlatformError::new(
                ErrorCode::ConfigInvalid,
                "failed to encode lock diagnostic metadata",
            )
        })?;
        // Write through the held fd so the advisory lock stays on the same inode.
        // Contents and PID are diagnostic only and are not treated as ownership.
        let mut file = &self.file;
        file.seek(SeekFrom::Start(0))
            .map_err(|_| PlatformError::new(ErrorCode::PathInvalid, "failed to seek lock file"))?;
        file.set_len(0).map_err(|_| {
            PlatformError::new(ErrorCode::PathInvalid, "failed to truncate lock file")
        })?;
        file.write_all(&json).map_err(|_| {
            PlatformError::new(ErrorCode::PathInvalid, "failed to write lock metadata")
        })?;
        file.sync_all().map_err(|_| {
            PlatformError::new(ErrorCode::PathInvalid, "failed to fsync lock metadata")
        })?;
        Ok(())
    }
}

/// Non-mutating exclusive flock held for the duration of doctor inspection.
///
/// Never writes lock metadata. Unlocks on drop.
#[derive(Debug)]
pub struct InspectLock {
    file: File,
}

impl InspectLock {
    /// Try to acquire a nonblocking exclusive lock without writing metadata.
    ///
    /// Returns `Ok(None)` when another owner holds the platform lock.
    pub fn try_acquire(path: &Path) -> Result<Option<Self>, PlatformError> {
        fs::require_absolute(path)?;
        let file = fs::open_nofollow(path, false, false).map_err(|_| {
            PlatformError::new(
                ErrorCode::PathInvalid,
                "failed to open data directory lock file",
            )
        })?;
        fs::validate_authority_fd(&file)?;
        match flock(&file, FlockOperation::NonBlockingLockExclusive) {
            Ok(()) => Ok(Some(Self { file })),
            Err(_) => Ok(None),
        }
    }
}

impl Drop for InspectLock {
    fn drop(&mut self) {
        let _ = flock(&self.file, FlockOperation::Unlock);
    }
}

impl Drop for DataDirLock {
    fn drop(&mut self) {
        let _ = flock(&self.file, FlockOperation::Unlock);
    }
}

fn classify_durability(path: &Path) -> FilesystemDurability {
    match rustix::fs::statfs(path) {
        Ok(stat) => classify_statfs(&stat),
        Err(_) => FilesystemDurability::Unclassified,
    }
}

#[cfg(target_os = "linux")]
fn classify_statfs(stat: &rustix::fs::StatFs) -> FilesystemDurability {
    const NFS: i64 = 0x6969;
    const CIFS: i64 = 0xFF5_34D42;
    const SMB: i64 = 0x517B;
    const FUSE: i64 = 0x6573_5546;
    const AFS: i64 = 0x5346_414F;
    let fs_type = stat.f_type;
    if matches!(fs_type, NFS | CIFS | SMB | FUSE | AFS) {
        FilesystemDurability::NetworkOrRemote
    } else {
        FilesystemDurability::ApparentlyLocal
    }
}

#[cfg(not(target_os = "linux"))]
fn classify_statfs(stat: &rustix::fs::StatFs) -> FilesystemDurability {
    let raw = stat.f_fstypename;
    let bytes: Vec<u8> = raw
        .iter()
        .copied()
        .take_while(|c| *c != 0)
        .map(|c| c as u8)
        .collect();
    let name = String::from_utf8_lossy(&bytes).to_ascii_lowercase();
    if name.contains("nfs")
        || name.contains("smb")
        || name.contains("afp")
        || name.contains("fuse")
        || name.contains("webdav")
        || name.contains("cifs")
    {
        FilesystemDurability::NetworkOrRemote
    } else if name.is_empty() {
        FilesystemDurability::Unclassified
    } else {
        FilesystemDurability::ApparentlyLocal
    }
}
