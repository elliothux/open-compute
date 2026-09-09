use super::*;

pub(crate) struct WorkDir {
    path: PathBuf,
}

impl WorkDir {
    pub(crate) fn create(parent: &Path, prefix: &str) -> Result<Self, PlatformError> {
        let parent_fd = open_dir_nofollow(parent)?;
        let name = format!("{prefix}.{}", uuid::Uuid::now_v7());
        mkdirat(&parent_fd, name.as_str(), Mode::RWXU).map_err(|_| {
            PlatformError::new(
                ErrorCode::ConfigCompileFailed,
                "failed to create compile workspace",
            )
        })?;
        let child = openat(
            &parent_fd,
            name.as_str(),
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|_| {
            PlatformError::new(
                ErrorCode::ConfigCompileFailed,
                "failed to create compile workspace",
            )
        })?;
        fchmod(&child, Mode::RWXU).map_err(|_| path_invalid("failed to set permissions"))?;
        Ok(Self {
            path: parent.join(name),
        })
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for WorkDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

/// Staging directory removed on drop unless `persist` is set.
pub(crate) struct StagingDir {
    path: PathBuf,
    persist: bool,
}

impl StagingDir {
    pub(crate) fn create(parent: &Path, prefix: &str) -> Result<Self, PlatformError> {
        let parent_fd = open_dir_nofollow(parent)?;
        let name = format!("{prefix}-{}", uuid::Uuid::now_v7());
        mkdirat(&parent_fd, name.as_str(), Mode::RWXU)
            .map_err(|_| path_invalid("failed to create release staging directory"))?;
        let child = openat(
            &parent_fd,
            name.as_str(),
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|_| path_invalid("failed to create release staging directory"))?;
        fchmod(&child, Mode::RWXU).map_err(|_| path_invalid("failed to set permissions"))?;
        Ok(Self {
            path: parent.join(name),
            persist: false,
        })
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn persist(&mut self) {
        self.persist = true;
    }
}

impl Drop for StagingDir {
    fn drop(&mut self) {
        if !self.persist {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}
