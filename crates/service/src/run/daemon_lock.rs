//! One advisory owner for an OCD scope, independent of instance data locks.

use open_compute_core::{ErrorCode, PlatformError};
use rustix::fs::{FlockOperation, Mode, OFlags, flock};
use std::fs::File;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::Path;

/// Held for the complete lifetime of the scoped daemon.
pub(crate) struct DaemonLock(File);

impl DaemonLock {
    pub(crate) fn acquire(root: &Path) -> Result<Self, PlatformError> {
        open_compute_storage::ensure_dir_secure(root)?;
        let fd = rustix::fs::open(
            root.join("ocd.lock"),
            OFlags::RDWR | OFlags::CREATE | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::from_raw_mode(0o600),
        )
        .map_err(|_| invalid("failed to open OCD lock"))?;
        Self::from_file(File::from(fd))
    }

    /// Acquire an existing scope without creating directories or writing lock metadata.
    pub(crate) fn acquire_existing(root: &Path) -> Result<Self, PlatformError> {
        Self::validate_root(root)?;
        let fd = rustix::fs::open(
            root.join("ocd.lock"),
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|_| invalid("OCD scope lock is unavailable"))?;
        Self::from_file(File::from(fd))
    }

    /// Create only the lock in an already restored scope directory.
    pub(crate) fn acquire_restored(root: &Path) -> Result<Self, PlatformError> {
        Self::validate_root(root)?;
        Self::acquire(root)
    }

    fn validate_root(root: &Path) -> Result<(), PlatformError> {
        let meta = std::fs::symlink_metadata(root)
            .map_err(|_| invalid("OCD scope directory is unavailable"))?;
        if !meta.file_type().is_dir()
            || meta.uid() != rustix::process::getuid().as_raw()
            || meta.permissions().mode() & 0o077 != 0
        {
            return Err(invalid("OCD scope directory owner or mode is invalid"));
        }
        Ok(())
    }

    fn from_file(file: File) -> Result<Self, PlatformError> {
        let metadata = file
            .metadata()
            .map_err(|_| invalid("failed to inspect OCD lock"))?;
        if !metadata.is_file()
            || metadata.uid() != rustix::process::getuid().as_raw()
            || metadata.permissions().mode() & 0o077 != 0
        {
            return Err(invalid("OCD lock must be an owner-only regular file"));
        }
        flock(&file, FlockOperation::NonBlockingLockExclusive)
            .map_err(|_| invalid("OCD scope is already owned by another daemon"))?;
        Ok(Self(file))
    }
}

impl Drop for DaemonLock {
    fn drop(&mut self) {
        let _ = flock(&self.0, FlockOperation::Unlock);
    }
}

fn invalid(message: &'static str) -> PlatformError {
    PlatformError::new(ErrorCode::InstanceRegistryInvalid, message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_owner_per_scope_and_no_lock_unlink() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("ocd");
        let other_root = temp.path().join("other-ocd");
        let first = DaemonLock::acquire(&root).unwrap();
        assert!(DaemonLock::acquire(&root).is_err());
        let other = DaemonLock::acquire(&other_root).unwrap();
        drop(other);
        assert!(root.join("ocd.lock").is_file());
        drop(first);
        let second = DaemonLock::acquire(&root).unwrap();
        assert!(root.join("ocd.lock").is_file());
        drop(second);
    }

    #[test]
    fn rejects_symlinked_or_group_readable_lock() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("ocd");
        open_compute_storage::ensure_dir_secure(&root).unwrap();
        let lock = root.join("ocd.lock");
        std::os::unix::fs::symlink(root.join("other"), &lock).unwrap();
        assert!(DaemonLock::acquire(&root).is_err());
        std::fs::remove_file(&lock).unwrap();
        std::fs::write(&lock, b"").unwrap();
        std::fs::set_permissions(&lock, std::fs::Permissions::from_mode(0o640)).unwrap();
        assert!(DaemonLock::acquire(&root).is_err());
    }

    #[test]
    fn existing_scope_lock_is_read_only_and_never_creates_root() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("ocd");
        assert!(DaemonLock::acquire_existing(&root).is_err());
        assert!(!root.exists());
        let live = DaemonLock::acquire(&root).unwrap();
        assert!(DaemonLock::acquire_existing(&root).is_err());
        drop(live);
        let old = std::fs::read(root.join("ocd.lock")).unwrap();
        let offline = DaemonLock::acquire_existing(&root).unwrap();
        assert_eq!(std::fs::read(root.join("ocd.lock")).unwrap(), old);
        drop(offline);
    }

    #[test]
    fn restored_scope_creates_only_its_missing_lock() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("ocd");
        assert!(DaemonLock::acquire_restored(&root).is_err());
        assert!(!root.exists());
        open_compute_storage::ensure_dir_secure(&root).unwrap();
        let live = DaemonLock::acquire_restored(&root).unwrap();
        assert!(root.join("ocd.lock").is_file());
        assert!(DaemonLock::acquire_restored(&root).is_err());
        drop(live);
        assert!(DaemonLock::acquire_existing(&root).is_ok());
    }
}
