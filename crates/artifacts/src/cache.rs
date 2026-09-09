//! Verified local artifact cache. The selected object backend remains the authority.

use crate::artifact::{ArtifactRef, parse_sha256};
use crate::error;
use crate::store::ArtifactStore;
use open_compute_core::{CacheConfig, ErrorCode, PlatformError, StartupId};
use rand::Rng;
use rustix::fd::{AsFd, OwnedFd};
use rustix::fs::{Mode, OFlags, fchmod, open, openat};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, VecDeque};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek};
#[cfg(target_os = "macos")]
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::{Duration, SystemTime};
use tokio::io::{AsyncRead, ReadBuf};
use tokio::sync::{Mutex as AsyncMutex, OnceCell};

const FILE_MODE: u32 = 0o600;

/// Readable cache handle that pins the entry against eviction.
#[derive(Debug)]
pub struct PinnedArtifact {
    file: File,
    _pin: Arc<()>,
}

/// Asynchronous reader that keeps its verified cache entry pinned until body completion.
#[derive(Debug)]
pub struct PinnedArtifactReader {
    file: tokio::fs::File,
    _pin: Arc<()>,
}

impl AsyncRead for PinnedArtifactReader {
    fn poll_read(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.file).poll_read(context, buffer)
    }
}

impl PinnedArtifact {
    /// Borrow the opened regular file.
    #[must_use]
    pub fn file(&self) -> &File {
        &self.file
    }

    /// Read the entire verified contents.
    pub fn read_all(&mut self) -> Result<Vec<u8>, PlatformError> {
        let mut buf = Vec::new();
        self.file.read_to_end(&mut buf).map_err(|_| {
            PlatformError::new(ErrorCode::PathInvalid, "failed to read cached artifact")
        })?;
        let _ = self.file.rewind();
        Ok(buf)
    }

    /// Convert into an async reader while preserving the eviction pin.
    #[must_use]
    pub fn into_async_reader(self) -> PinnedArtifactReader {
        PinnedArtifactReader {
            file: tokio::fs::File::from_std(self.file),
            _pin: self._pin,
        }
    }
}

#[derive(Debug)]
struct EntryMeta {
    size: u64,
    pin: Arc<()>,
    verified: bool,
}

#[derive(Debug)]
struct CacheInner {
    entries: HashMap<String, EntryMeta>,
    lru: VecDeque<String>,
    total_bytes: u64,
}

type InflightMap = HashMap<String, Arc<OnceCell<Result<ArtifactRef, PlatformError>>>>;

/// Local verified cache rooted at an explicit absolute directory.
#[derive(Debug)]
pub struct ArtifactCache {
    root: PathBuf,
    config: CacheConfig,
    startup_id: StartupId,
    inner: Arc<Mutex<CacheInner>>,
    inflight: AsyncMutex<InflightMap>,
}

mod backend;

struct PartialGuard {
    path: PathBuf,
    persist: bool,
}

impl Drop for PartialGuard {
    fn drop(&mut self) {
        if !self.persist {
            let _ = fs::remove_file(&self.path);
        }
    }
}

fn libc_nofollow() -> i32 {
    OFlags::NOFOLLOW.bits() as i32
}

fn cache_path(root: &Path, digest: &str) -> PathBuf {
    root.join("sha256").join(&digest[..2]).join(&digest[2..])
}

fn validate_cache_root(root: &Path) -> Result<(), PlatformError> {
    if !root.is_absolute() {
        return Err(PlatformError::new(
            ErrorCode::PathInvalid,
            "cache root must be an absolute path",
        ));
    }
    if root
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err(PlatformError::new(
            ErrorCode::PathInvalid,
            "cache root must not contain '..'",
        ));
    }
    Ok(())
}

fn path_invalid(msg: &'static str) -> PlatformError {
    PlatformError::new(ErrorCode::PathInvalid, msg)
}

#[cfg(target_os = "macos")]
fn is_macos_root_system_alias(parent: &OwnedFd, name: &std::ffi::OsStr, target: &[u8]) -> bool {
    let expected = match name.as_bytes() {
        b"tmp" => b"private/tmp".as_slice(),
        b"var" => b"private/var".as_slice(),
        _ => return false,
    };
    if target != expected {
        return false;
    }
    let Ok(root) = open(
        "/",
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
    ) else {
        return false;
    };
    let Ok(parent_stat) = rustix::fs::fstat(parent) else {
        return false;
    };
    let Ok(root_stat) = rustix::fs::fstat(&root) else {
        return false;
    };
    parent_stat.st_dev == root_stat.st_dev && parent_stat.st_ino == root_stat.st_ino
}

fn open_existing_component(
    parent: &OwnedFd,
    name: &std::ffi::OsStr,
) -> Result<OwnedFd, PlatformError> {
    match openat(
        parent,
        name,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    ) {
        Ok(child) => Ok(child),
        Err(err) if err == rustix::io::Errno::LOOP || err == rustix::io::Errno::NOTDIR => {
            let target = rustix::fs::readlinkat(parent, name, Vec::new())
                .map_err(|_| path_invalid("cache path must be a real directory"))?;
            #[cfg(not(target_os = "macos"))]
            {
                let _ = target;
                Err(path_invalid("cache path must be a real directory"))
            }
            #[cfg(target_os = "macos")]
            {
                if !is_macos_root_system_alias(parent, name, target.as_bytes()) {
                    return Err(path_invalid("cache path must be a real directory"));
                }
                openat(
                    parent,
                    name,
                    OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
                    Mode::empty(),
                )
                .map_err(|_| path_invalid("cache path must be a real directory"))
            }
        }
        Err(_) => Err(path_invalid("cache path must be a real directory")),
    }
}

fn open_dir_nofollow(path: &Path, create: bool) -> Result<OwnedFd, PlatformError> {
    let mut fd = open(
        "/",
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|_| path_invalid("failed to open cache directory"))?;
    let names: Vec<_> = path
        .components()
        .filter_map(|c| match c {
            std::path::Component::Normal(name) => Some(name),
            _ => None,
        })
        .collect();
    let last = names.len().saturating_sub(1);
    for (i, name) in names.into_iter().enumerate() {
        let is_last = i == last;
        match openat(
            &fd,
            name,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        ) {
            Ok(child) => fd = child,
            Err(err) if err == rustix::io::Errno::NOENT && create => {
                match rustix::fs::mkdirat(&fd, name, Mode::RWXU) {
                    Ok(()) => {}
                    Err(exist) if exist == rustix::io::Errno::EXIST => {}
                    Err(_) => return Err(path_invalid("failed to create cache directory")),
                }
                fd = openat(
                    &fd,
                    name,
                    OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                    Mode::empty(),
                )
                .map_err(|_| path_invalid("cache path must be a real directory"))?;
            }
            Err(err)
                if !is_last
                    && (err == rustix::io::Errno::LOOP || err == rustix::io::Errno::NOTDIR) =>
            {
                fd = open_existing_component(&fd, name)?;
            }
            Err(_) => {
                return Err(path_invalid("cache path must be a real directory"));
            }
        }
    }
    Ok(fd)
}

fn ensure_real_dir(path: &Path) -> Result<(), PlatformError> {
    let fd = open_dir_nofollow(path, true)?;
    fchmod(&fd, Mode::RWXU).map_err(|_| path_invalid("failed to set cache permissions"))
}

fn ensure_child_dir(parent: &Path, name: &str) -> Result<(), PlatformError> {
    let parent_fd = open_dir_nofollow(parent, false)?;
    match rustix::fs::mkdirat(&parent_fd, name, Mode::RWXU) {
        Ok(()) => {}
        Err(err) if err == rustix::io::Errno::EXIST => {}
        Err(_) => {
            return Err(path_invalid("failed to create cache directory"));
        }
    }
    let child = openat(
        &parent_fd,
        name,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|_| path_invalid("cache directory must not be a symlink"))?;
    fchmod(&child, Mode::RWXU).map_err(|_| path_invalid("failed to set cache permissions"))
}

fn fsync_dir(path: &Path) -> Result<(), PlatformError> {
    let dir = open(
        path,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|_| PlatformError::new(ErrorCode::PathInvalid, "failed to open cache directory"))?;
    rustix::fs::fsync(dir.as_fd())
        .map_err(|_| PlatformError::new(ErrorCode::PathInvalid, "failed to fsync cache directory"))
}

fn open_entry_fd(path: &Path) -> std::io::Result<File> {
    OpenOptions::new()
        .read(true)
        .custom_flags(libc_nofollow())
        .open(path)
}

fn hash_fd(file: &mut File, artifact: &ArtifactRef) -> Result<(), PlatformError> {
    #[cfg(test)]
    test_hooks::run_hash_pause();
    let mut hasher = Sha256::new();
    let mut buf = [0_u8; 8192];
    let mut total = 0_u64;
    loop {
        let n = file.read(&mut buf).map_err(|_| {
            PlatformError::new(
                ErrorCode::CacheEntryCorrupt,
                "cache entry could not be read",
            )
        })?;
        if n == 0 {
            break;
        }
        total += n as u64;
        hasher.update(&buf[..n]);
    }
    if total != artifact.size() {
        return Err(PlatformError::new(
            ErrorCode::CacheEntryCorrupt,
            "cache entry size mismatch",
        ));
    }
    if hasher.finalize().as_slice() != artifact.sha256_bytes() {
        return Err(PlatformError::new(
            ErrorCode::CacheEntryCorrupt,
            "cache entry digest mismatch",
        ));
    }
    Ok(())
}

fn is_safe_evict_target(path: &Path) -> bool {
    let Ok(meta) = fs::symlink_metadata(path) else {
        return false;
    };
    if meta.file_type().is_symlink() || !meta.file_type().is_file() {
        return false;
    }
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    !name.starts_with('.')
}

fn cleanup_stale_partials(sha_root: &Path, grace: Duration) {
    let now = SystemTime::now();
    let Ok(shards) = fs::read_dir(sha_root) else {
        return;
    };
    for shard in shards.flatten() {
        let path = shard.path();
        let Ok(meta) = fs::symlink_metadata(&path) else {
            continue;
        };
        if meta.file_type().is_symlink() || !meta.file_type().is_dir() {
            continue;
        }
        let Ok(ents) = fs::read_dir(&path) else {
            continue;
        };
        for ent in ents.flatten() {
            let p = ent.path();
            let name = ent.file_name();
            let name = name.to_string_lossy();
            if !name.starts_with(".partial.") {
                continue;
            }
            let Ok(meta) = fs::symlink_metadata(&p) else {
                continue;
            };
            if meta.file_type().is_symlink() {
                continue;
            }
            let Ok(modified) = meta.modified() else {
                continue;
            };
            if now.duration_since(modified).unwrap_or_default() > grace {
                let _ = fs::remove_file(&p);
            }
        }
    }
}

fn rebuild_index(sha_root: &Path) -> CacheInner {
    let mut inner = CacheInner {
        entries: HashMap::new(),
        lru: VecDeque::new(),
        total_bytes: 0,
    };
    let Ok(shards) = fs::read_dir(sha_root) else {
        return inner;
    };
    let mut found = Vec::new();
    for shard in shards.flatten() {
        let path = shard.path();
        let Ok(meta) = fs::symlink_metadata(&path) else {
            continue;
        };
        if meta.file_type().is_symlink() || !meta.file_type().is_dir() {
            continue;
        }
        let shard_name = shard.file_name();
        let shard_str = shard_name.to_string_lossy();
        if shard_str.len() != 2 {
            continue;
        }
        let Ok(ents) = fs::read_dir(&path) else {
            continue;
        };
        for ent in ents.flatten() {
            let name = ent.file_name();
            let rest = name.to_string_lossy();
            if rest.starts_with('.') || rest.len() != 62 {
                continue;
            }
            let digest = format!("{shard_str}{rest}");
            if parse_sha256(&digest).is_err() {
                continue;
            }
            let p = ent.path();
            let Ok(meta) = fs::symlink_metadata(&p) else {
                continue;
            };
            if meta.file_type().is_symlink() || !meta.file_type().is_file() {
                continue;
            }
            let mtime = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
            found.push((mtime, digest, meta.len()));
        }
    }
    found.sort_by_key(|(t, _, _)| *t);
    for (_, digest, size) in found {
        inner.total_bytes = inner.total_bytes.saturating_add(size);
        inner.entries.insert(
            digest.clone(),
            EntryMeta {
                size,
                pin: Arc::new(()),
                verified: false,
            },
        );
        inner.lru.push_back(digest);
    }
    inner
}

#[cfg(test)]
pub(crate) mod test_hooks {
    use super::{ArtifactCache, ArtifactRef, PinnedArtifact};
    use open_compute_core::PlatformError;
    use std::sync::{Arc, Mutex, OnceLock};

    type HashPauseFn = Arc<dyn Fn() + Send + Sync>;
    type HashPauseSlot = Mutex<Option<HashPauseFn>>;

    static HASH_PAUSE: OnceLock<HashPauseSlot> = OnceLock::new();

    fn hash_pause() -> &'static HashPauseSlot {
        HASH_PAUSE.get_or_init(|| Mutex::new(None))
    }

    pub(crate) struct HashPauseGuard;

    impl Drop for HashPauseGuard {
        fn drop(&mut self) {
            if let Ok(mut slot) = hash_pause().lock() {
                *slot = None;
            }
        }
    }

    pub(crate) fn install_hash_pause(hook: HashPauseFn) -> HashPauseGuard {
        *hash_pause().lock().expect("hash pause lock") = Some(hook);
        HashPauseGuard
    }

    pub(crate) fn run_hash_pause() {
        let hook = hash_pause().lock().ok().and_then(|g| g.clone());
        if let Some(hook) = hook {
            hook();
        }
    }

    impl ArtifactCache {
        pub(crate) fn try_hit_for_test(
            &self,
            artifact: &ArtifactRef,
        ) -> Result<Option<PinnedArtifact>, PlatformError> {
            self.try_hit(artifact)
        }

        pub(crate) fn is_indexed_for_test(&self, digest: &str) -> bool {
            self.inner
                .lock()
                .ok()
                .is_some_and(|g| g.entries.contains_key(digest))
        }
    }
}

#[cfg(test)]
#[path = "cache_tests.rs"]
mod coverage_tests;
