//! Durable non-secret fences for uncertain credential writes, proxy removals and captures.

use super::contract::{invalid, opaque};
use open_compute_core::PlatformError;
use open_compute_storage::{fs, lock::InspectLock};
use std::{
    collections::BTreeSet,
    fs::{File, OpenOptions},
    io::Read as _,
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    sync::Mutex,
};

pub(super) struct Journal {
    directory: PathBuf,
    pending: Mutex<BTreeSet<String>>,
    _lock: InspectLock,
}

impl Journal {
    pub(super) fn open(directory: &Path, maximum: u32) -> Result<Self, PlatformError> {
        super::socket::validate_directory(directory, rustix::process::getuid().as_raw())?;
        let lock_path = directory.join("broker.lock");
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&lock_path)
        {
            Ok(file) => {
                file.sync_all().map_err(|_| invalid())?;
                sync(directory)?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(_) => return Err(invalid()),
        }
        fs::validate_owned_file(&lock_path)?;
        let lock = InspectLock::try_acquire(&lock_path)?.ok_or_else(invalid)?;
        let mut pending = BTreeSet::new();
        for entry in std::fs::read_dir(directory).map_err(|_| invalid())? {
            let entry = entry.map_err(|_| invalid())?;
            let name = entry.file_name();
            let name = name.to_str().ok_or_else(invalid)?;
            if name == "broker.lock" {
                continue;
            }
            let key = name
                .strip_prefix("pending-")
                .filter(|key| opaque(key))
                .ok_or_else(invalid)?;
            fs::validate_owned_file(&entry.path())?;
            let fd = rustix::fs::open(
                entry.path(),
                rustix::fs::OFlags::RDONLY
                    | rustix::fs::OFlags::NOFOLLOW
                    | rustix::fs::OFlags::CLOEXEC,
                rustix::fs::Mode::empty(),
            )
            .map_err(|_| invalid())?;
            let file = File::from(fd);
            if !file.metadata().map_err(|_| invalid())?.is_file() {
                return Err(invalid());
            }
            let mut content = Vec::with_capacity(4);
            file.take(4)
                .read_to_end(&mut content)
                .map_err(|_| invalid())?;
            if content != b"v1\n" || pending.len() >= maximum as usize {
                return Err(invalid());
            }
            pending.insert(key.to_owned());
        }
        Ok(Self {
            directory: directory.to_owned(),
            pending: Mutex::new(pending),
            _lock: lock,
        })
    }

    pub(super) fn keys(&self) -> Result<BTreeSet<String>, PlatformError> {
        Ok(self.pending.lock().map_err(|_| invalid())?.clone())
    }

    pub(super) fn contains(&self, key: &str) -> Result<bool, PlatformError> {
        Ok(self.pending.lock().map_err(|_| invalid())?.contains(key))
    }

    pub(super) fn begin(&self, key: &str) -> Result<(), PlatformError> {
        if !opaque(key) {
            return Err(invalid());
        }
        let mut pending = self.pending.lock().map_err(|_| invalid())?;
        if pending.contains(key) {
            return Err(invalid());
        }
        // This durable marker must precede the first engine write byte.
        fs::atomic_write(&self.directory.join(format!("pending-{key}")), b"v1\n")?;
        pending.insert(key.to_owned());
        Ok(())
    }

    pub(super) fn clear(&self, key: &str) -> Result<(), PlatformError> {
        let mut pending = self.pending.lock().map_err(|_| invalid())?;
        if pending.contains(key) {
            let path = self.directory.join(format!("pending-{key}"));
            fs::validate_owned_file(&path)?;
            std::fs::remove_file(path).map_err(|_| invalid())?;
            sync(&self.directory)?;
            pending.remove(key);
        }
        Ok(())
    }
}

fn sync(directory: &Path) -> Result<(), PlatformError> {
    File::open(directory)
        .and_then(|file| file.sync_all())
        .map_err(|_| invalid())
}
