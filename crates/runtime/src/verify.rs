//! Open the packaged workerd binary and check `--version`.

use crate::fsutil::{open_file, require_absolute};
use crate::lock::RuntimeLock;
use crate::process::{assert_reaped, run_verified_fd, run_verified_fd_with_lease};
use open_compute_core::{ErrorCode, PlatformError, Redactor};
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

const VERSION_STDOUT_LIMIT: usize = 4096;
const VERSION_ARG: &str = "--version";

/// Verified, secret-safe workerd identity bound to the opened executable.
pub struct VerifiedRuntime {
    target: String,
    release: String,
    binary_sha256: String,
    version_output: String,
    lock: RuntimeLock,
    lock_bytes: Vec<u8>,
    file: Arc<File>,
    staging_lease_path: Option<PathBuf>,
    pyodide_bundle_cache_dir: Option<PathBuf>,
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
            file: self.file.clone(),
            staging_lease_path: self.staging_lease_path.clone(),
            pyodide_bundle_cache_dir: self.pyodide_bundle_cache_dir.clone(),
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
            && self.staging_lease_path == other.staging_lease_path
            && self.pyodide_bundle_cache_dir == other.pyodide_bundle_cache_dir
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

    /// Build-time binary SHA-256 pin, lowercase hex.
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

    /// Require the native private Container attachment protocol from this verified executable.
    pub async fn check_container_protocol(
        &self,
        deadline: Duration,
        redactor: &Redactor,
    ) -> Result<(), PlatformError> {
        let output = self
            .run(
                &["container-protocol-version"],
                deadline,
                128,
                redactor,
                None,
            )
            .await?;
        if !output.status.is_some_and(|status| status.success())
            || output.timed_out
            || output.stdout_overflow
            || output.stderr_overflow
            || output.stdout != b"open-compute-container-protocol-v4\n"
        {
            return Err(PlatformError::new(
                ErrorCode::RuntimeInvalid,
                "verified runtime lacks the required native Container attachment protocol",
            ));
        }
        Ok(())
    }

    /// Opened executable. Never a caller pathname.
    pub(crate) fn executable_file(&self) -> &File {
        self.file.as_ref()
    }

    pub(crate) fn pyodide_bundle_cache_dir(&self) -> Option<&Path> {
        self.pyodide_bundle_cache_dir.as_deref()
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
                    self.file.as_ref(),
                    path,
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
                    self.file.as_ref(),
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
    pyodide_bundle_cache_dir: Option<&Path>,
) -> Result<VerifiedRuntime, PlatformError> {
    require_absolute(binary)?;
    let lock = RuntimeLock::parse(lock_bytes)?;
    let (target_name, target) = lock.current_target()?;

    let file = open_file(binary, false, false).map_err(|_| {
        PlatformError::new(
            ErrorCode::RuntimeInvalid,
            "runtime binary is missing or not a regular file",
        )
    })?;
    let binary_sha256 = target.binary_sha256.clone();
    if let Some(path) = staging_lease_path {
        crate::lease::recover_orphans(path)?;
    }

    let output = match staging_lease_path {
        Some(path) => {
            run_verified_fd_with_lease(
                &file,
                path,
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
        file: Arc::new(file),
        staging_lease_path: staging_lease_path.map(Path::to_path_buf),
        pyodide_bundle_cache_dir: pyodide_bundle_cache_dir.map(Path::to_path_buf),
    })
}
