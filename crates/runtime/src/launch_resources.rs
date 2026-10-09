//! Already-opened ancillary resources for host executables with adjacent assets.

use crate::process::{ExecImage, VerifiedLaunchImage};
use open_compute_core::{ErrorCode, PlatformError};
use std::fs::File;
use std::os::unix::fs::PermissionsExt;

impl VerifiedLaunchImage {
    /// Transfer verified sibling resource files together with an opened executable.
    /// The owning domain verifies digests and capabilities before calling this method.
    pub fn from_verified_resources(
        file: File,
        resources: Vec<(String, File)>,
    ) -> Result<Self, PlatformError> {
        let mut names = std::collections::BTreeSet::new();
        let mut total = 0u64;
        if resources.len() > 64 {
            return Err(invalid());
        }
        for (name, resource) in &resources {
            if name.is_empty()
                || name.len() > 128
                || name == "workerd"
                || !name
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
                || matches!(name.as_str(), "." | "..")
                || !names.insert(name)
            {
                return Err(invalid());
            }
            let metadata = resource.metadata().map_err(|_| invalid())?;
            if !metadata.is_file() || metadata.permissions().mode() & 0o022 != 0 {
                return Err(invalid());
            }
            total = total.checked_add(metadata.len()).ok_or_else(invalid)?;
            if total > 256 * 1024 * 1024 {
                return Err(invalid());
            }
        }
        Ok(Self { file, resources })
    }
}

impl ExecImage {
    #[cfg_attr(
        not(target_os = "macos"),
        allow(
            clippy::unnecessary_wraps,
            reason = "macOS stages verified sibling resources fallibly"
        )
    )]
    pub(crate) fn stage_resources(
        &self,
        resources: &[(String, File)],
    ) -> Result<(), PlatformError> {
        // Linux executes the original opened inode through /proc/self/fd; its installation
        // resources stay adjacent. macOS posix_spawn requires the private staged image.
        #[cfg(target_os = "macos")]
        if !resources.is_empty() {
            use std::io::{Read, Seek};
            let root = self.program.parent().ok_or_else(invalid)?;
            for (name, source) in resources {
                let mut source = source.try_clone().map_err(|_| invalid())?;
                source.rewind().map_err(|_| invalid())?;
                let mut target = crate::fsutil::open_nofollow(&root.join(name), true, true)?;
                let expected = source.metadata().map_err(|_| invalid())?.len();
                let written = std::io::copy(&mut source.take(expected + 1), &mut target)
                    .map_err(|_| invalid())?;
                if written != expected {
                    return Err(invalid());
                }
                target.sync_all().map_err(|_| invalid())?;
                target
                    .set_permissions(std::fs::Permissions::from_mode(0o400))
                    .map_err(|_| invalid())?;
            }
        }
        #[cfg(not(target_os = "macos"))]
        let _ = resources;
        Ok(())
    }
}

fn invalid() -> PlatformError {
    PlatformError::new(
        ErrorCode::RuntimeInvalid,
        "invalid verified executable resources",
    )
}

/// Remove only bounded regular files in an owned private launch directory, never nested trees.
#[cfg(any(test, target_os = "macos"))]
pub(crate) fn remove_staged_files(directory: &std::path::Path) -> Result<(), PlatformError> {
    use rustix::fs::{AtFlags, FileType, statat, unlinkat};
    use std::os::unix::ffi::OsStrExt;
    let dir = crate::fsutil::open_dir_nofollow(directory)?;
    let entries = rustix::fs::Dir::read_from(&dir).map_err(|_| invalid())?;
    let mut names = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|_| invalid())?;
        let bytes = entry.file_name().to_bytes();
        if matches!(bytes, b"." | b"..") {
            continue;
        }
        if names.len() >= 65 {
            return Err(invalid());
        }
        let name = std::ffi::OsStr::from_bytes(bytes).to_os_string();
        let metadata = statat(&dir, &name, AtFlags::SYMLINK_NOFOLLOW).map_err(|_| invalid())?;
        if FileType::from_raw_mode(metadata.st_mode) != FileType::RegularFile {
            return Err(invalid());
        }
        names.push(name);
    }
    for name in names {
        unlinkat(&dir, &name, AtFlags::empty()).map_err(|_| invalid())?;
    }
    Ok(())
}
