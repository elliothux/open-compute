//! Secure per-request staging for streamed R2 uploads.

use crate::fs;
use open_compute_core::{ErrorCode, PlatformError, ResourceId};
use std::fs::File;
use std::path::{Path, PathBuf};

const R2_STAGING_DIR: &str = "r2-staging";

/// Canonical host-owned staging layout for R2 uploads.
#[derive(Clone, Debug)]
pub struct R2Staging {
    root: PathBuf,
}

impl R2Staging {
    /// Create and validate `<data>/r2-staging` on first use.
    pub fn open(data_root: &Path) -> Result<Self, PlatformError> {
        fs::require_absolute(data_root)?;
        fs::validate_root(data_root)?;
        let root = data_root.join(R2_STAGING_DIR);
        fs::create_dir_secure(&root)?;
        fs::validate_owned_dir(&root)?;
        fs::validate_contained(data_root, &root)?;
        Ok(Self { root })
    }

    /// Create one exclusive 0600 regular file from typed host identities only.
    pub fn create(
        &self,
        resource: ResourceId,
        request_id: &str,
    ) -> Result<(PathBuf, File), PlatformError> {
        validate_uuid(request_id)?;
        let resource_dir = self.root.join(resource.to_string());
        fs::create_dir_secure(&resource_dir)?;
        fs::validate_owned_dir(&resource_dir)?;
        fs::validate_contained(&self.root, &resource_dir)?;
        let path = resource_dir.join(request_id);
        let flags = rustix::fs::OFlags::CREATE
            | rustix::fs::OFlags::EXCL
            | rustix::fs::OFlags::NOFOLLOW
            | rustix::fs::OFlags::CLOEXEC
            | rustix::fs::OFlags::RDWR;
        let mode = rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR;
        let fd = rustix::fs::open(&path, flags, mode).map_err(|_| staging_error())?;
        let file = File::from(fd);
        fs::validate_authority_fd(&file)?;
        fs::validate_owned_file(&path, true)?;
        Ok((path, file))
    }

    /// Remove stale canonical files during single-owner startup.
    ///
    /// Unknown entries make startup fail closed instead of being followed or
    /// removed as if they were platform-owned files.
    pub fn cleanup(&self) -> Result<u32, PlatformError> {
        let mut removed = 0_u32;
        for resource in std::fs::read_dir(&self.root).map_err(|_| staging_error())? {
            let resource = resource.map_err(|_| staging_error())?;
            let name = resource.file_name();
            let Some(name) = name.to_str() else {
                return Err(staging_error());
            };
            validate_uuid(name)?;
            let kind = resource.file_type().map_err(|_| staging_error())?;
            if kind.is_symlink() || !kind.is_dir() {
                return Err(staging_error());
            }
            let directory = resource.path();
            fs::validate_owned_dir(&directory)?;
            for request in std::fs::read_dir(&directory).map_err(|_| staging_error())? {
                let request = request.map_err(|_| staging_error())?;
                let request_name = request.file_name();
                let Some(request_name) = request_name.to_str() else {
                    return Err(staging_error());
                };
                validate_uuid(request_name)?;
                let kind = request.file_type().map_err(|_| staging_error())?;
                if kind.is_symlink() || !kind.is_file() {
                    return Err(staging_error());
                }
                fs::validate_owned_file(&request.path(), true)?;
                std::fs::remove_file(request.path()).map_err(|_| staging_error())?;
                removed = removed.saturating_add(1);
            }
            std::fs::remove_dir(&directory).map_err(|_| staging_error())?;
        }
        fs::fsync_dir(&self.root)?;
        Ok(removed)
    }
}

fn validate_uuid(value: &str) -> Result<(), PlatformError> {
    let id = uuid::Uuid::parse_str(value).map_err(|_| staging_error())?;
    if id.hyphenated().to_string() != value {
        return Err(staging_error());
    }
    Ok(())
}

fn staging_error() -> PlatformError {
    PlatformError::new(
        ErrorCode::PathInvalid,
        "R2 upload staging authority is invalid",
    )
}

#[cfg(test)]
#[path = "r2_staging_tests.rs"]
mod tests;
