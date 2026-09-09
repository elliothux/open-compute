//! Bounded argv process execution with TERM/KILL/reap of the process group.

use open_compute_core::{ErrorCode, PlatformError, Redactor};
use rustix::io::{FdFlags, fcntl_getfd, fcntl_setfd};
use rustix::process::{
    Pid, Signal, getpgid, kill_process, kill_process_group, test_kill_process,
    test_kill_process_group,
};
use std::ffi::OsStr;
use std::fs::{self, File};
#[cfg(target_os = "macos")]
use std::io::Seek;
use std::io::{Read, Write};
#[cfg(target_os = "linux")]
use std::os::fd::AsRawFd;
use std::os::fd::{AsFd, OwnedFd};
#[cfg(target_os = "macos")]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::oneshot;

const MAX_STDERR: usize = 64 * 1024;
const KILL_GRACE: Duration = Duration::from_millis(200);

/// Result of a bounded child execution. Stderr is already redacted.
#[derive(Debug)]
pub struct BoundedOutput {
    /// Process exit status, if the child exited.
    pub status: Option<std::process::ExitStatus>,
    /// Bounded stdout bytes.
    pub stdout: Vec<u8>,
    /// Bounded, redacted stderr.
    pub stderr: Vec<u8>,
    /// True if the deadline fired before exit.
    pub timed_out: bool,
    /// True if stdout exceeded the configured bound.
    pub stdout_overflow: bool,
    /// Child PID that was waited, if spawn succeeded.
    pub pid: Option<i32>,
}

/// Keep-alive executable identity used for fd-based spawn.
#[derive(Debug)]
pub(crate) struct ExecImage {
    _keep: File,
    _staging: Option<PathBuf>,
    _staging_journal: Option<PathBuf>,
    pub(crate) program: PathBuf,
}

/// Duplicate `file` without `CLOEXEC` and map it to a kernel fd path.
pub(crate) fn exec_image(file: &File) -> Result<ExecImage, PlatformError> {
    exec_image_inner(file, None)
}

/// Materialize an executable while journaling macOS staging next to `lease_path`.
pub(crate) fn exec_image_with_lease(
    file: &File,
    lease_path: &Path,
    binary_sha256: &str,
) -> Result<ExecImage, PlatformError> {
    exec_image_inner(file, Some((lease_path, binary_sha256)))
}

fn exec_image_inner(
    file: &File,
    staging_lease: Option<(&Path, &str)>,
) -> Result<ExecImage, PlatformError> {
    let owned: OwnedFd = rustix::io::dup(file.as_fd()).map_err(|_| {
        PlatformError::new(
            ErrorCode::RuntimeInvalid,
            "failed to duplicate verified executable fd",
        )
    })?;
    let mut flags = fcntl_getfd(&owned).map_err(|_| {
        PlatformError::new(
            ErrorCode::RuntimeInvalid,
            "failed to read verified executable fd flags",
        )
    })?;
    flags.remove(FdFlags::CLOEXEC);
    fcntl_setfd(&owned, flags).map_err(|_| {
        PlatformError::new(
            ErrorCode::RuntimeInvalid,
            "failed to clear CLOEXEC on verified executable fd",
        )
    })?;
    let mut keep = File::from(owned);
    let (program, staging, staging_journal) = exec_path_for(&mut keep, staging_lease)?;
    Ok(ExecImage {
        _keep: keep,
        _staging: staging,
        _staging_journal: staging_journal,
        program,
    })
}

impl Drop for ExecImage {
    fn drop(&mut self) {
        if let Some(dir) = self._staging.take() {
            cleanup_staging_dir(&dir);
        }
        if let Some(journal) = self._staging_journal.take() {
            let _ = fs::remove_file(journal);
        }
    }
}

#[cfg_attr(
    not(target_os = "macos"),
    allow(
        clippy::unnecessary_wraps,
        reason = "the shared signature must carry errors from the fallible macOS implementation"
    )
)]
fn exec_path_for(
    file: &mut File,
    staging_lease: Option<(&Path, &str)>,
) -> Result<(PathBuf, Option<PathBuf>, Option<PathBuf>), PlatformError> {
    #[cfg(target_os = "linux")]
    {
        let raw = file.as_raw_fd();
        let _ = staging_lease;
        Ok((PathBuf::from(format!("/proc/self/fd/{raw}")), None, None))
    }
    #[cfg(target_os = "macos")]
    {
        // posix_spawn CLOEXEC_DEFAULT drops extra fds, so /dev/fd/N cannot be
        // exec'd. Copy the already-opened vnode into a private exclusive file
        // and execute that path. This never reopens the caller pathname.
        let staging = std::env::temp_dir().join(format!("oc-exec-{}", uuid::Uuid::now_v7()));
        let staging_journal = staging_lease
            .map(|(lease_path, digest)| write_staging_journal(lease_path, &staging, digest))
            .transpose()?;
        let materialized = (|| {
            fs::create_dir(&staging).map_err(|_| {
                PlatformError::new(
                    ErrorCode::RuntimeInvalid,
                    "failed to create verified executable staging directory",
                )
            })?;
            let mut perms = fs::metadata(&staging)
                .map_err(|_| {
                    PlatformError::new(
                        ErrorCode::RuntimeInvalid,
                        "failed to create verified executable staging directory",
                    )
                })?
                .permissions();
            perms.set_mode(0o700);
            fs::set_permissions(&staging, perms).map_err(|_| {
                PlatformError::new(
                    ErrorCode::RuntimeInvalid,
                    "failed to create verified executable staging directory",
                )
            })?;
            let dest = staging.join("workerd");
            {
                let mut out = crate::fsutil::open_nofollow(&dest, true, true).map_err(|_| {
                    PlatformError::new(
                        ErrorCode::RuntimeInvalid,
                        "failed to materialize verified executable",
                    )
                })?;
                file.rewind().map_err(|_| {
                    PlatformError::new(
                        ErrorCode::RuntimeInvalid,
                        "failed to rewind verified executable",
                    )
                })?;
                let mut buf = [0u8; 8192];
                loop {
                    let n = file.read(&mut buf).map_err(|_| {
                        PlatformError::new(
                            ErrorCode::RuntimeInvalid,
                            "failed to read verified executable",
                        )
                    })?;
                    if n == 0 {
                        break;
                    }
                    out.write_all(&buf[..n]).map_err(|_| {
                        PlatformError::new(
                            ErrorCode::RuntimeInvalid,
                            "failed to materialize verified executable",
                        )
                    })?;
                }
                out.sync_all().map_err(|_| {
                    PlatformError::new(
                        ErrorCode::RuntimeInvalid,
                        "failed to fsync verified executable",
                    )
                })?;
            }
            let mut dest_perms = fs::metadata(&dest)
                .map_err(|_| {
                    PlatformError::new(
                        ErrorCode::RuntimeInvalid,
                        "failed to materialize verified executable",
                    )
                })?
                .permissions();
            dest_perms.set_mode(0o700);
            fs::set_permissions(&dest, dest_perms).map_err(|_| {
                PlatformError::new(
                    ErrorCode::RuntimeInvalid,
                    "failed to materialize verified executable",
                )
            })?;
            Ok::<_, PlatformError>(dest)
        })();
        let dest = match materialized {
            Ok(dest) => dest,
            Err(error) => {
                if cleanup_staging_dir_strict(&staging).is_ok()
                    && let Some(journal) = &staging_journal
                {
                    let _ = fs::remove_file(journal);
                }
                return Err(error);
            }
        };
        Ok((dest, Some(staging), staging_journal))
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = (file, staging_lease);
        Err(PlatformError::new(
            ErrorCode::RuntimeInvalid,
            "fd execution is not supported on this OS",
        ))
    }
}

#[cfg(target_os = "macos")]
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StagingJournal {
    schema_version: u32,
    directory: PathBuf,
    binary_sha256: String,
}

#[cfg(target_os = "macos")]
fn write_staging_journal(
    lease_path: &Path,
    directory: &Path,
    binary_sha256: &str,
) -> Result<PathBuf, PlatformError> {
    let path = staging_journal_path(lease_path);
    let bytes = serde_json::to_vec(&StagingJournal {
        schema_version: 1,
        directory: directory.to_owned(),
        binary_sha256: binary_sha256.to_owned(),
    })
    .map_err(|_| {
        PlatformError::new(
            ErrorCode::RuntimeInvalid,
            "failed to encode runtime staging journal",
        )
    })?;
    crate::fsutil::write_atomic_replace(&path, &bytes, 0o600)?;
    Ok(path)
}

pub(crate) fn staging_journal_path(lease_path: &Path) -> PathBuf {
    lease_path.with_extension("staging")
}

pub(crate) fn clear_staging_journal(lease_path: &Path) -> Result<(), PlatformError> {
    let path = staging_journal_path(lease_path);
    crate::fsutil::remove_file_nofollow(&path)
}

#[cfg_attr(
    not(target_os = "macos"),
    allow(
        clippy::unnecessary_wraps,
        reason = "the shared signature must carry errors from the fallible macOS implementation"
    )
)]
pub(crate) fn recover_unleased_staging(
    lease_path: &Path,
    expected_digest: &str,
) -> Result<(), PlatformError> {
    #[cfg(target_os = "macos")]
    {
        let journal_path = staging_journal_path(lease_path);
        let Some(mut file) = crate::fsutil::open_optional_nofollow(&journal_path)? else {
            return Ok(());
        };
        let metadata = file.metadata().map_err(|_| {
            PlatformError::new(
                ErrorCode::PathInvalid,
                "failed to stat runtime staging journal",
            )
        })?;
        if !metadata.is_file() || metadata.len() > 4096 {
            return Err(PlatformError::new(
                ErrorCode::RuntimeInvalid,
                "runtime staging journal is not a bounded regular file",
            ));
        }
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).map_err(|_| {
            PlatformError::new(
                ErrorCode::PathInvalid,
                "failed to read runtime staging journal",
            )
        })?;
        let journal: StagingJournal = serde_json::from_slice(&bytes).map_err(|_| {
            PlatformError::new(
                ErrorCode::RuntimeInvalid,
                "runtime staging journal is malformed",
            )
        })?;
        if journal.schema_version != 1
            || journal.binary_sha256 != expected_digest
            || !private_staging_dir(&journal.directory)
        {
            return Err(PlatformError::new(
                ErrorCode::RuntimeInvalid,
                "runtime staging journal does not match the verified runtime",
            ));
        }
        let executable = journal.directory.join("workerd");
        let executable = executable.canonicalize().unwrap_or(executable);
        let mut user = None;
        for _ in 0..10 {
            user = executable_user(&executable)?;
            if user.is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        if user.is_some() {
            return Err(PlatformError::new(
                ErrorCode::RuntimeInvalid,
                "unleased runtime staging executable is still in use",
            ));
        }
        cleanup_staging_dir_strict(&journal.directory)?;
        clear_staging_journal(lease_path)?;
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (lease_path, expected_digest);
        Ok(())
    }
}

#[cfg(target_os = "macos")]
fn private_staging_dir(directory: &Path) -> bool {
    let Some(name) = directory.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    let Some(uuid) = name.strip_prefix("oc-exec-") else {
        return false;
    };
    let Some(parent) = directory.parent() else {
        return false;
    };
    uuid::Uuid::parse_str(uuid).is_ok()
        && match (parent.canonicalize(), std::env::temp_dir().canonicalize()) {
            (Ok(parent), Ok(temp)) => parent == temp,
            _ => false,
        }
}

#[cfg(target_os = "macos")]
fn executable_user(path: &Path) -> Result<Option<i32>, PlatformError> {
    if !path.exists() {
        return Ok(None);
    }
    let output = std::process::Command::new("/usr/sbin/lsof")
        .args(["-nP", "-d", "txt", "-Fn"])
        .stdin(Stdio::null())
        .output()
        .map_err(|_| {
            PlatformError::new(
                ErrorCode::RuntimeInvalid,
                "failed to inspect runtime staging executable ownership",
            )
        })?;
    match output.status.code() {
        Some(0) => {
            let target = path.canonicalize().map_err(|_| {
                PlatformError::new(
                    ErrorCode::RuntimeInvalid,
                    "failed to canonicalize runtime staging executable",
                )
            })?;
            let mut current_pid = None;
            let mut pids = Vec::new();
            for line in String::from_utf8_lossy(&output.stdout).lines() {
                if let Some(pid) = line.strip_prefix('p') {
                    current_pid = Some(pid.parse::<i32>().map_err(|_| {
                        PlatformError::new(
                            ErrorCode::RuntimeInvalid,
                            "runtime staging ownership output was malformed",
                        )
                    })?);
                } else if let Some(name) = line.strip_prefix('n')
                    && Path::new(name).file_name() == target.file_name()
                    && Path::new(name).canonicalize().ok().as_ref() == Some(&target)
                    && let Some(pid) = current_pid
                {
                    pids.push(pid);
                }
            }
            pids.sort_unstable();
            pids.dedup();
            match pids.as_slice() {
                [] => Ok(None),
                [pid] => Ok(Some(*pid)),
                _ => Err(PlatformError::new(
                    ErrorCode::RuntimeInvalid,
                    "runtime staging executable has ambiguous ownership",
                )),
            }
        }
        Some(1) => Ok(None),
        _ => Err(PlatformError::new(
            ErrorCode::RuntimeInvalid,
            "failed to inspect runtime staging executable ownership",
        )),
    }
}

fn cleanup_staging_dir(directory: &Path) {
    let _ = fs::remove_file(directory.join("workerd"));
    let _ = fs::remove_dir(directory);
}

#[cfg(any(test, target_os = "macos"))]
fn cleanup_staging_dir_strict(directory: &Path) -> Result<(), PlatformError> {
    match fs::symlink_metadata(directory) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Ok(_) => {}
        Err(_) => {
            return Err(PlatformError::new(
                ErrorCode::PathInvalid,
                "failed to inspect runtime staging directory",
            ));
        }
    }
    crate::fsutil::remove_file_nofollow(&directory.join("workerd"))?;
    crate::fsutil::remove_empty_dir_nofollow(directory)
}

/// Execute the already-opened verified file. Never reopens a caller pathname.
mod execution;
mod signals;

pub(crate) use execution::*;
pub use signals::*;

#[cfg(test)]
#[test]
fn fallback_skips_signal_after_owner_reaps() {
    let mut cmd = std::process::Command::new("/bin/sleep");
    std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
    let child = cmd
        .arg("30")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn");
    let pid = child.id() as i32;
    let mut owned = OwnedChild::new(child, pid);
    owned.fail_safe_kill();
    owned.disarm();
    drop(owned);
    wait_reaped(pid, Duration::from_secs(2)).expect("owner reaped without a fallback signal");
}

#[cfg(all(test, target_os = "macos"))]
#[test]
fn staging_journal_recovers_interrupted_copy_without_child_lease() {
    let data = tempfile::TempDir::new().expect("temporary runtime data");
    let lease_path = data.path().join("child.lease");
    let digest = "ab".repeat(32);
    let staging = std::env::temp_dir().join(format!("oc-exec-{}", uuid::Uuid::now_v7()));
    fs::create_dir(&staging).expect("create interrupted staging directory");
    fs::write(staging.join("workerd"), b"partial verified executable copy")
        .expect("write interrupted copy");
    let journal = write_staging_journal(&lease_path, &staging, &digest).expect("write journal");

    recover_unleased_staging(&lease_path, &digest).expect("recover interrupted staging");

    assert!(!staging.exists(), "interrupted staging directory leaked");
    assert!(!journal.exists(), "staging journal leaked");
}

#[cfg(all(test, target_os = "macos"))]
#[test]
fn staging_journal_recovers_crash_before_directory_creation() {
    let data = tempfile::TempDir::new().expect("temporary runtime data");
    let lease_path = data.path().join("child.lease");
    let digest = "ab".repeat(32);
    let staging = std::env::temp_dir().join(format!("oc-exec-{}", uuid::Uuid::now_v7()));
    assert!(!staging.exists());
    let journal = write_staging_journal(&lease_path, &staging, &digest).expect("write journal");

    recover_unleased_staging(&lease_path, &digest).expect("recover empty staging journal");

    assert!(!staging.exists());
    assert!(!journal.exists(), "empty staging journal leaked");
}

#[cfg(all(test, target_os = "macos"))]
#[test]
fn complete_staging_without_child_lease_is_recovered() {
    use sha2::Digest as _;

    let data = tempfile::TempDir::new().expect("temporary runtime data");
    let lease_path = data.path().join("child.lease");
    let staging = std::env::temp_dir().join(format!("oc-exec-{}", uuid::Uuid::now_v7()));
    fs::create_dir(&staging).expect("create complete staging directory");
    let executable = staging.join("workerd");
    let bytes = b"complete verified executable copy";
    fs::write(&executable, bytes).expect("write complete copy");
    let digest = hex::encode(sha2::Sha256::digest(bytes));
    let journal = write_staging_journal(&lease_path, &staging, &digest).expect("write journal");

    recover_unleased_staging(&lease_path, &digest).expect("recover complete staging");

    assert!(!staging.exists(), "complete unleased staging leaked");
    assert!(
        !journal.exists(),
        "complete unleased staging journal leaked"
    );
}

#[cfg(test)]
#[test]
fn process_helpers_fail_closed_on_absent_and_invalid_processes() {
    let data = tempfile::TempDir::new().expect("temporary data");
    let lease = data.path().join("child.lease");
    assert_eq!(
        staging_journal_path(&lease),
        data.path().join("child.staging")
    );
    clear_staging_journal(&lease).expect("missing journal is already clear");
    cleanup_staging_dir_strict(&data.path().join("missing")).expect("missing staging is clear");

    assert!(!process_group_live(0));
    assert_reaped(None).expect("no pid is reaped");
    assert_reaped(Some(0)).expect("invalid pid is treated as absent");
    wait_pid_gone(0, Duration::ZERO).expect("invalid pid is absent");
    terminate_group_term(None);
    terminate_group_term(Some(0));
    terminate_group_kill(None);
    terminate_group_kill(Some(0));

    let pipe = PipeState::new();
    assert!(pipe.take_error().is_none());
    assert!(pipe.take_bytes().is_empty());
    join_readers(None, None, std::time::Instant::now()).expect("no readers");
    let panicking = std::thread::spawn(|| panic!("reader failure"));
    assert!(join_readers(Some(panicking), None, std::time::Instant::now()).is_err());

    let mut owned = OwnedChild {
        child: None,
        pid: 0,
        disarmed: false,
    };
    assert!(owned.take_stdout().is_none());
    assert!(owned.take_stderr().is_none());
    assert!(owned.try_wait().expect("empty owner").is_none());
    assert!(owned.wait().expect("empty owner").is_none());
    let mut status = Some(
        std::process::Command::new("/usr/bin/true")
            .status()
            .unwrap(),
    );
    let mut error = None;
    reap_after_kill(&mut owned, &mut status, &mut error);
    assert!(error.is_none());
    owned.disarm();

    let mut guard = ProcessGuard {
        cancel: None,
        owner: None,
    };
    guard.disarm();
}

#[cfg(all(test, target_os = "macos"))]
#[test]
fn staging_journal_validation_matrix_is_fail_closed() {
    let data = tempfile::TempDir::new().expect("temporary data");
    let lease = data.path().join("child.lease");
    let journal = staging_journal_path(&lease);
    let digest = "ab".repeat(32);

    assert!(!private_staging_dir(Path::new("relative")));
    assert!(!private_staging_dir(Path::new("/")));
    assert!(!private_staging_dir(
        &std::env::temp_dir().join("wrong-prefix")
    ));
    assert!(!private_staging_dir(
        &std::env::temp_dir().join("oc-exec-not-a-uuid")
    ));
    assert_eq!(
        executable_user(&data.path().join("missing")).expect("missing executable"),
        None
    );

    fs::write(&journal, b"not json").unwrap();
    assert!(recover_unleased_staging(&lease, &digest).is_err());
    fs::write(&journal, vec![b'x'; 4097]).unwrap();
    assert!(recover_unleased_staging(&lease, &digest).is_err());

    for body in [
        serde_json::json!({
            "schemaVersion": 2,
            "directory": std::env::temp_dir().join(format!("oc-exec-{}", uuid::Uuid::now_v7())),
            "binarySha256": digest,
        }),
        serde_json::json!({
            "schemaVersion": 1,
            "directory": std::env::temp_dir().join(format!("oc-exec-{}", uuid::Uuid::now_v7())),
            "binarySha256": "cd".repeat(32),
        }),
        serde_json::json!({
            "schemaVersion": 1,
            "directory": data.path().join("not-private"),
            "binarySha256": digest,
        }),
    ] {
        fs::write(&journal, serde_json::to_vec(&body).unwrap()).unwrap();
        assert!(recover_unleased_staging(&lease, &digest).is_err());
    }
    fs::remove_file(&journal).unwrap();

    let staging = std::env::temp_dir().join(format!("oc-exec-{}", uuid::Uuid::now_v7()));
    fs::create_dir(&staging).unwrap();
    fs::write(staging.join("unexpected"), b"keep").unwrap();
    assert!(cleanup_staging_dir_strict(&staging).is_err());
    fs::remove_file(staging.join("unexpected")).unwrap();
    fs::remove_dir(&staging).unwrap();
}

#[cfg(test)]
#[path = "process_tests.rs"]
mod coverage_tests;
