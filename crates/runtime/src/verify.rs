//! Open a pinned workerd binary, hash it, and verify `--version`.

use crate::fsutil::{
    hash_file, hex_sha256, open_nofollow, parse_sha256_hex, require_absolute, require_executable_fd,
};
use crate::lock::RuntimeLock;
use crate::process::{assert_reaped, run_verified_fd, run_verified_fd_with_lease};
use open_compute_core::{ErrorCode, PlatformError, Redactor};
use std::fs::File;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

const VERSION_STDOUT_LIMIT: usize = 4096;
const VERSION_ARG: &str = "--version";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct BinaryIdentity {
    dev: u64,
    ino: u64,
    size: u64,
    mtime_secs: i64,
    mtime_nsecs: i64,
}

static HASH_CACHE: Mutex<Option<(BinaryIdentity, [u8; 32])>> = Mutex::new(None);

/// Verified, secret-safe workerd identity bound to the opened executable.
pub struct VerifiedRuntime {
    target: String,
    release: String,
    binary_sha256: String,
    version_output: String,
    lock: RuntimeLock,
    lock_bytes: Vec<u8>,
    file: File,
    pub(crate) expected_assets_sha256: Option<&'static str>,
    staging_lease_path: Option<PathBuf>,
}

impl Clone for VerifiedRuntime {
    fn clone(&self) -> Self {
        Self {
            target: self.target.clone(),
            release: self.release.clone(),
            binary_sha256: self.binary_sha256.clone(),
            version_output: self.version_output.clone(),
            lock: self.lock.clone(),
            lock_bytes: self.lock_bytes.clone(),
            file: self.file.try_clone().expect("dup verified executable fd"),
            expected_assets_sha256: self.expected_assets_sha256,
            staging_lease_path: self.staging_lease_path.clone(),
        }
    }
}

impl std::fmt::Debug for VerifiedRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VerifiedRuntime")
            .field("target", &self.target)
            .field("release", &self.release)
            .field("binary_sha256", &self.binary_sha256)
            .field("version_output", &self.version_output)
            .finish_non_exhaustive()
    }
}

impl PartialEq for VerifiedRuntime {
    fn eq(&self, other: &Self) -> bool {
        self.target == other.target
            && self.release == other.release
            && self.binary_sha256 == other.binary_sha256
            && self.version_output == other.version_output
            && self.lock == other.lock
            && self.lock_bytes == other.lock_bytes
            && self.expected_assets_sha256 == other.expected_assets_sha256
            && self.staging_lease_path == other.staging_lease_path
    }
}

impl Eq for VerifiedRuntime {}

impl VerifiedRuntime {
    /// Lock target name, for example `darwin-arm64`.
    #[must_use]
    pub fn target(&self) -> &str {
        &self.target
    }

    /// Release tag from the lock.
    #[must_use]
    pub fn release(&self) -> &str {
        &self.release
    }

    /// SHA-256 of the verified binary, lowercase hex.
    #[must_use]
    pub fn binary_sha256(&self) -> &str {
        &self.binary_sha256
    }

    /// Exact version stdout, trimmed.
    #[must_use]
    pub fn version_output(&self) -> &str {
        &self.version_output
    }

    /// Parsed lock identity bound at verification time.
    #[must_use]
    pub fn lock(&self) -> &RuntimeLock {
        &self.lock
    }

    /// Exact lock file bytes bound at verification time.
    #[must_use]
    pub fn lock_bytes(&self) -> &[u8] {
        &self.lock_bytes
    }

    /// Opened executable. Never a caller pathname.
    pub(crate) fn executable_file(&self) -> &File {
        &self.file
    }

    /// Spawn the verified executable with explicit argv. Never accepts an arbitrary path.
    pub async fn run(
        &self,
        args: &[&str],
        deadline: Duration,
        max_stdout: usize,
        redactor: &Redactor,
        stdout_file: Option<File>,
    ) -> Result<crate::process::BoundedOutput, PlatformError> {
        match &self.staging_lease_path {
            Some(path) => {
                run_verified_fd_with_lease(
                    &self.file,
                    path,
                    &self.binary_sha256,
                    args,
                    deadline,
                    max_stdout,
                    redactor,
                    stdout_file,
                )
                .await
            }
            None => {
                run_verified_fd(
                    &self.file,
                    args,
                    deadline,
                    max_stdout,
                    redactor,
                    stdout_file,
                )
                .await
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn with_binary_sha256(mut self, value: String) -> Self {
        self.binary_sha256 = value;
        self
    }
}

/// Verify `binary` against the lock file at `lock_path` and execute `workerd --version`.
///
/// Never downloads, never inspects `PATH`, and never follows a symlink.
#[cfg(any(test, feature = "test-support"))]
pub async fn verify_runtime_binary(
    lock_path: &Path,
    binary: &Path,
    deadline: Duration,
    redactor: &Redactor,
) -> Result<VerifiedRuntime, PlatformError> {
    let (_, bytes) = crate::lock::load_runtime_lock(lock_path)?;
    verify_runtime_binary_inner(&bytes, binary, deadline, redactor, None, None).await
}

pub(crate) async fn verify_runtime_binary_inner(
    lock_bytes: &[u8],
    binary: &Path,
    deadline: Duration,
    redactor: &Redactor,
    staging_lease_path: Option<&Path>,
    expected_assets_sha256: Option<&'static str>,
) -> Result<VerifiedRuntime, PlatformError> {
    require_absolute(binary)?;
    let lock = RuntimeLock::parse(lock_bytes)?;
    let (target_name, target) = lock.current_target()?;
    let expected = parse_sha256_hex(&target.binary_sha256)?;

    let mut file = open_nofollow(binary, false, false).map_err(|err| {
        if err.code() == ErrorCode::PathInvalid
            && err.message() == "path must not have a symlink ancestor"
        {
            err
        } else {
            PlatformError::new(
                ErrorCode::RuntimeInvalid,
                "runtime binary is missing or not a regular file",
            )
        }
    })?;
    let meta = require_executable_fd(&file)?;
    let identity = identity_from_meta(&meta);
    let digest = hash_cached(&mut file, identity)?;
    if digest != expected {
        return Err(PlatformError::new(
            ErrorCode::RuntimeInvalid,
            "runtime binary hash does not match the lock",
        ));
    }

    let binary_sha256 = hex_sha256(&digest);
    if let Some(path) = staging_lease_path {
        crate::lease::recover_orphans(path, &binary_sha256)?;
    }

    let output = match staging_lease_path {
        Some(path) => {
            run_verified_fd_with_lease(
                &file,
                path,
                &binary_sha256,
                &[VERSION_ARG],
                deadline,
                VERSION_STDOUT_LIMIT,
                redactor,
                None,
            )
            .await?
        }
        None => {
            run_verified_fd(
                &file,
                &[VERSION_ARG],
                deadline,
                VERSION_STDOUT_LIMIT,
                redactor,
                None,
            )
            .await?
        }
    };
    assert_reaped(output.pid)?;

    if output.timed_out {
        return Err(PlatformError::new(
            ErrorCode::RuntimeInvalid,
            "workerd version probe timed out",
        ));
    }
    if output.stdout_overflow {
        return Err(PlatformError::new(
            ErrorCode::RuntimeInvalid,
            "workerd version output exceeded the bound",
        ));
    }
    let status = output.status.ok_or_else(|| {
        PlatformError::new(
            ErrorCode::RuntimeInvalid,
            "workerd version probe did not exit",
        )
    })?;
    if !status.success() {
        return Err(PlatformError::new(
            ErrorCode::RuntimeInvalid,
            "workerd version probe exited unsuccessfully",
        ));
    }
    let stdout = std::str::from_utf8(&output.stdout).map_err(|_| {
        PlatformError::new(
            ErrorCode::RuntimeInvalid,
            "workerd version output is not UTF-8",
        )
    })?;
    let trimmed = stdout.trim();
    if trimmed != lock.expected_version_output {
        return Err(PlatformError::new(
            ErrorCode::RuntimeInvalid,
            "workerd version output does not match the lock",
        ));
    }

    Ok(VerifiedRuntime {
        target: target_name.to_owned(),
        release: lock.release.clone(),
        binary_sha256,
        version_output: trimmed.to_owned(),
        lock,
        lock_bytes: lock_bytes.to_vec(),
        file,
        expected_assets_sha256,
        staging_lease_path: staging_lease_path.map(Path::to_path_buf),
    })
}

fn identity_from_meta(meta: &std::fs::Metadata) -> BinaryIdentity {
    BinaryIdentity {
        dev: meta.dev(),
        ino: meta.ino(),
        size: meta.size(),
        mtime_secs: meta.mtime(),
        mtime_nsecs: meta.mtime_nsec(),
    }
}

fn hash_cached(file: &mut File, identity: BinaryIdentity) -> Result<[u8; 32], PlatformError> {
    let mut cache = HASH_CACHE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some((cached_id, digest)) = *cache
        && cached_id == identity
    {
        return Ok(digest);
    }
    let digest = hash_file(file)?;
    *cache = Some((identity, digest));
    Ok(digest)
}

#[cfg(test)]
pub(crate) fn clear_hash_cache() {
    let mut cache = HASH_CACHE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    *cache = None;
}
