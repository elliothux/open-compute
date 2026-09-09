use super::*;

pub(super) fn key_lock_index(key: &ObjectKey) -> usize {
    usize::from(Sha256::digest(key.as_str().as_bytes())[0]) % 64
}

pub(super) fn write_json_create(
    parent: &OwnedFd,
    name: &str,
    value: &impl Serialize,
) -> Result<(), BackendError> {
    let bytes = serde_json::to_vec(value).map_err(|_| BackendError::Corrupt)?;
    let fd = create_regular(parent, name)?;
    let mut file = File::from(fd);
    file.write_all(&bytes)
        .and_then(|()| file.sync_all())
        .map_err(|_| BackendError::Unavailable)
}

pub(super) fn write_json_replace(
    parent: &OwnedFd,
    name: &str,
    value: &impl Serialize,
) -> Result<(), BackendError> {
    let bytes = serde_json::to_vec(value).map_err(|_| BackendError::Corrupt)?;
    if bytes.len() > HEADER_BYTES {
        return Err(BackendError::Corrupt);
    }
    let partial = format!(".partial-{}", uuid::Uuid::now_v7());
    let mut guard = PartialGuard::new(dup_fd(parent)?, partial.clone());
    let fd = create_regular(parent, &partial)?;
    let mut file = File::from(fd);
    file.write_all(&bytes)
        .and_then(|()| file.sync_all())
        .map_err(|_| BackendError::Unavailable)?;
    renameat(parent, partial.as_str(), parent, name).map_err(|_| BackendError::Unavailable)?;
    guard.persist = true;
    fsync(parent.as_fd()).map_err(|_| BackendError::Unavailable)
}

pub(super) fn create_regular(parent: &OwnedFd, name: &str) -> Result<OwnedFd, BackendError> {
    openat(
        parent,
        name,
        OFlags::RDWR | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::RUSR | Mode::WUSR,
    )
    .map_err(|_| BackendError::Unavailable)
}

pub(super) fn open_regular(parent: &OwnedFd, name: &str) -> Result<OwnedFd, BackendError> {
    let fd = openat(
        parent,
        name,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK,
        Mode::empty(),
    )
    .map_err(|error| {
        if error == rustix::io::Errno::NOENT {
            BackendError::NotFound
        } else if error == rustix::io::Errno::LOOP
            || error == rustix::io::Errno::NOTDIR
            || statat(parent, name, AtFlags::SYMLINK_NOFOLLOW).is_ok_and(|stat| {
                rustix::fs::FileType::from_raw_mode(stat.st_mode)
                    != rustix::fs::FileType::RegularFile
            })
        {
            BackendError::Corrupt
        } else {
            BackendError::Unavailable
        }
    })?;
    validate_regular(&fd, None)?;
    Ok(fd)
}

pub(super) fn validate_regular(
    fd: &OwnedFd,
    expected_size: Option<u64>,
) -> Result<(), BackendError> {
    let stat = fstat(fd).map_err(|_| BackendError::Unavailable)?;
    if rustix::fs::FileType::from_raw_mode(stat.st_mode) != rustix::fs::FileType::RegularFile
        || stat.st_nlink != 1
        || stat.st_mode as u32 & 0o777 != 0o600
        || stat.st_uid != rustix::process::getuid().as_raw()
        || expected_size.is_some_and(|expected| stat.st_size as u64 != expected)
    {
        return Err(BackendError::Corrupt);
    }
    Ok(())
}

pub(super) fn validate_dir(fd: &OwnedFd) -> Result<(), BackendError> {
    let stat = fstat(fd).map_err(|_| BackendError::Unavailable)?;
    if rustix::fs::FileType::from_raw_mode(stat.st_mode) != rustix::fs::FileType::Directory
        || stat.st_mode as u32 & 0o777 != 0o700
        || stat.st_uid != rustix::process::getuid().as_raw()
    {
        return Err(BackendError::Corrupt);
    }
    Ok(())
}

pub(super) fn ensure_dir(parent: &OwnedFd, name: &str) -> Result<(), BackendError> {
    let created = match mkdirat(parent, name, Mode::RWXU) {
        Ok(()) => true,
        Err(error) if error == rustix::io::Errno::EXIST => false,
        Err(_) => return Err(BackendError::Unavailable),
    };
    let child = open_child_dir(parent, name)?;
    if created {
        fchmod(&child, Mode::RWXU).map_err(|_| BackendError::Unavailable)?;
    }
    validate_dir(&child)
}

pub(super) fn open_child_dir<P: rustix::path::Arg>(
    parent: &OwnedFd,
    name: P,
) -> Result<OwnedFd, BackendError> {
    let fd = openat(
        parent,
        name,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|error| {
        if error == rustix::io::Errno::NOENT {
            BackendError::NotFound
        } else if error == rustix::io::Errno::LOOP || error == rustix::io::Errno::NOTDIR {
            BackendError::Corrupt
        } else {
            BackendError::Unavailable
        }
    })?;
    validate_dir(&fd)?;
    Ok(fd)
}

pub(super) fn dir_names(directory: &OwnedFd) -> Result<Vec<OsString>, BackendError> {
    let entries = rustix::fs::Dir::read_from(directory).map_err(|_| BackendError::Unavailable)?;
    let mut names = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|_| BackendError::Unavailable)?;
        let bytes = entry.file_name().to_bytes();
        if bytes != b"." && bytes != b".." {
            if names.len() >= MAX_SCAN_ENTRIES {
                return Err(BackendError::Capacity);
            }
            names.push(OsString::from_vec(bytes.to_vec()));
        }
    }
    names.sort();
    Ok(names)
}

pub(super) fn dup_fd(fd: &OwnedFd) -> Result<OwnedFd, BackendError> {
    rustix::io::dup(fd).map_err(|_| BackendError::Unavailable)
}

pub(super) struct PartialGuard {
    parent: OwnedFd,
    name: String,
    pub(super) persist: bool,
}

impl PartialGuard {
    pub(super) fn new(parent: OwnedFd, name: String) -> Self {
        Self {
            parent,
            name,
            persist: false,
        }
    }
}

impl Drop for PartialGuard {
    fn drop(&mut self) {
        if !self.persist {
            let _ = unlinkat(&self.parent, self.name.as_str(), AtFlags::empty());
        }
    }
}

pub(super) fn platform_integrity(_error: BackendError) -> PlatformError {
    PlatformError::new(
        ErrorCode::ObjectStorageIntegrityError,
        "local object authority failed integrity validation",
    )
}

pub(super) fn platform_unavailable() -> PlatformError {
    PlatformError::new(
        ErrorCode::ObjectStorageUnavailable,
        "object storage authority is unavailable",
    )
}

pub(super) fn require_local_filesystem(fd: &impl rustix::fd::AsFd) -> Result<(), PlatformError> {
    let stat = rustix::fs::fstatfs(fd).map_err(|_| platform_unavailable())?;
    if filesystem_is_network_or_unknown(&stat) {
        return Err(PlatformError::new(
            ErrorCode::ConfigInvalid,
            "local object authority requires a classified local filesystem",
        ));
    }
    Ok(())
}

#[cfg(target_os = "linux")]
pub(super) fn filesystem_is_network_or_unknown(stat: &rustix::fs::StatFs) -> bool {
    const NFS: i64 = 0x6969;
    const CIFS: i64 = 0xFF5_34D42;
    const SMB: i64 = 0x517B;
    const FUSE: i64 = 0x6573_5546;
    const AFS: i64 = 0x5346_414F;
    matches!(stat.f_type, NFS | CIFS | SMB | FUSE | AFS)
}

#[cfg(not(target_os = "linux"))]
pub(super) fn filesystem_is_network_or_unknown(stat: &rustix::fs::StatFs) -> bool {
    let raw = stat.f_fstypename;
    let bytes: Vec<u8> = raw
        .iter()
        .copied()
        .take_while(|byte| *byte != 0)
        .map(|byte| byte as u8)
        .collect();
    let name = String::from_utf8_lossy(&bytes).to_ascii_lowercase();
    name.is_empty()
        || name.contains("nfs")
        || name.contains("smb")
        || name.contains("afp")
        || name.contains("fuse")
        || name.contains("webdav")
        || name.contains("cifs")
}
