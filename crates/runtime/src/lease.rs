//! Secret-free child process lease for next-start orphan recovery.
//!
//! Production identity is OS start time (`/proc/<pid>/stat` or exact
//! `/bin/ps -o lstart=`). Tests may inject the identity reader. Recovery never
//! signals a process that fails live PID/PGID/leader/start/digest checks.

#[cfg(target_os = "macos")]
use crate::fsutil::open_nofollow;
use crate::fsutil::{
    hash_file, hex_sha256, open_optional_nofollow, require_absolute, write_atomic_replace,
};
use crate::process::{assert_reaped, clear_staging_journal, recover_unleased_staging};
use open_compute_core::{ErrorCode, PlatformError};
use rustix::process::{
    Pid, Signal, WaitId, WaitIdOptions, getpgid, kill_process_group, test_kill_process, waitid,
};
use serde::{Deserialize, Serialize};
use std::io::Read;
use std::path::Path;
#[cfg(any(target_os = "macos", test))]
use std::process::{Command, Stdio};
use std::time::Duration;

#[cfg(any(test, feature = "test-support"))]
use std::sync::Mutex;

const SCHEMA: u32 = 1;
const REAP_DEADLINE: Duration = Duration::from_secs(5);
const MAX_LEASE_BYTES: u64 = 4096;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct ChildLease {
    pub schema_version: u32,
    pub pid: i32,
    pub pgid: i32,
    pub start_key: String,
    pub binary_sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub staged_executable: Option<String>,
}

#[cfg(any(test, feature = "test-support"))]
type StartKeyFn = fn(i32) -> Option<String>;

#[cfg(any(test, feature = "test-support"))]
static START_KEY_HOOK: Mutex<Option<StartKeyFn>> = Mutex::new(None);

#[cfg(any(test, feature = "test-support"))]
static WRITE_FAIL: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Test-only identity reader. Production always uses OS start time.
#[cfg(any(test, feature = "test-support"))]
pub fn set_start_key_hook(hook: Option<StartKeyFn>) {
    *START_KEY_HOOK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = hook;
}

/// Test-only injected `write_lease` failure.
#[cfg(any(test, feature = "test-support"))]
pub fn set_lease_write_fail(fail: bool) {
    WRITE_FAIL.store(fail, std::sync::atomic::Ordering::SeqCst);
}

pub(crate) fn capture_lease(pid: i32, pgid: i32, binary_sha256: &str) -> Option<ChildLease> {
    if pid <= 1 || pgid <= 1 || pid != pgid {
        return None;
    }
    let start_key = read_start_key(pid)?;
    Some(ChildLease {
        schema_version: SCHEMA,
        pid,
        pgid,
        start_key,
        binary_sha256: binary_sha256.to_owned(),
        staged_executable: staged_executable_path(pid).and_then(|path| {
            private_staging_path(&path).then(|| path.to_string_lossy().into_owned())
        }),
    })
}

pub(crate) fn write_lease(path: &Path, lease: &ChildLease) -> Result<(), PlatformError> {
    #[cfg(any(test, feature = "test-support"))]
    if WRITE_FAIL.load(std::sync::atomic::Ordering::SeqCst) {
        return Err(PlatformError::new(
            ErrorCode::RuntimeInvalid,
            "injected lease write failure",
        ));
    }
    require_absolute(path)?;
    let bytes = serde_json::to_vec(lease).map_err(|_| {
        PlatformError::new(ErrorCode::RuntimeInvalid, "failed to encode child lease")
    })?;
    write_atomic_replace(path, &bytes, 0o600)
}

pub(crate) fn clear_lease(path: &Path) -> Result<(), PlatformError> {
    require_absolute(path)?;
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(PlatformError::new(
            ErrorCode::PathInvalid,
            "failed to remove child lease",
        )),
    }
}

/// Recover a verified orphan. `Ok(None)` means no matching live child.
/// `Err` is fail-closed: do not spawn a replacement.
pub(crate) fn recover_orphans(
    path: &Path,
    expected_digest: &str,
) -> Result<Option<i32>, PlatformError> {
    require_absolute(path)?;
    let Some(lease) = load_lease(path)? else {
        recover_unleased_staging(path, expected_digest)?;
        return Ok(None);
    };
    if lease.pid > 1 {
        let raw = Pid::from_raw(lease.pid)
            .ok_or_else(|| recovery_refused("child lease PID is invalid"))?;
        match test_kill_process(raw) {
            Err(err) if err == rustix::io::Errno::SRCH => {
                cleanup_dead_staging(&lease, expected_digest)?;
                clear_staging_journal(path)?;
                clear_lease(path)?;
                return Ok(None);
            }
            Ok(()) => {}
            Err(_) => {
                return Err(recovery_refused(
                    "child lease PID could not be verified; refusing to continue",
                ));
            }
        }
    }
    if lease.schema_version != SCHEMA
        || lease.pid <= 1
        || lease.pgid <= 1
        || lease.pid != lease.pgid
        || lease.binary_sha256 != expected_digest
        || lease.start_key.is_empty()
    {
        return Err(recovery_refused(
            "child lease is invalid or does not match the verified runtime",
        ));
    }
    match live_match(&lease, expected_digest) {
        LiveMatch::Gone => {
            cleanup_dead_staging(&lease, expected_digest)?;
            clear_staging_journal(path)?;
            clear_lease(path)?;
            Ok(None)
        }
        LiveMatch::Mismatch => Err(recovery_refused(
            "live child does not match its lease; refusing to signal or continue",
        )),
        LiveMatch::IdentityUnavailable => Err(recovery_refused(
            "child lease identity could not be verified; refusing to signal or continue",
        )),
        LiveMatch::Verified(staged_executable) => {
            signal_verified_group(lease.pgid)?;
            wait_leader_and_group(lease.pid, lease.pgid)?;
            if let Some(staged_executable) = staged_executable {
                cleanup_staging(&staged_executable, expected_digest)?;
            }
            clear_staging_journal(path)?;
            clear_lease(path)?;
            Ok(Some(lease.pid))
        }
    }
}

/// Fail closed when an offline operation observes a live or unverifiable runtime child.
///
/// This check never signals a process or removes lease/staging evidence. A dead lease is safe for
/// an offline reader and will be reclaimed by the next supervised daemon start.
pub fn assert_no_live_orphan(path: &Path, expected_digest: &str) -> Result<(), PlatformError> {
    require_absolute(path)?;
    let Some(lease) = load_lease(path)? else {
        if std::fs::symlink_metadata(crate::process::staging_journal_path(path)).is_ok() {
            return Err(recovery_refused(
                "unleased runtime staging evidence requires supervised recovery",
            ));
        }
        return Ok(());
    };
    let Some(pid) = Pid::from_raw(lease.pid) else {
        return Ok(());
    };
    match test_kill_process(pid) {
        Err(error) if error == rustix::io::Errno::SRCH => Ok(()),
        Err(_) => Err(recovery_refused(
            "child lease PID could not be verified; offline operation refused",
        )),
        Ok(()) => match live_match(&lease, expected_digest) {
            LiveMatch::Gone => Ok(()),
            LiveMatch::Verified(_) => Err(PlatformError::new(
                ErrorCode::PlatformUnavailable,
                "verified workerd child is still live; offline operation refused",
            )),
            LiveMatch::Mismatch | LiveMatch::IdentityUnavailable => Err(recovery_refused(
                "live child does not match verifiable lease identity; offline operation refused",
            )),
        },
    }
}

/// Recover a formally identified orphan from a process-level integration test.
#[cfg(any(test, feature = "test-support"))]
pub fn recover_orphan_for_test(
    path: &Path,
    expected_digest: &str,
) -> Result<Option<i32>, PlatformError> {
    recover_orphans(path, expected_digest)
}

fn recovery_refused(message: &'static str) -> PlatformError {
    PlatformError::new(ErrorCode::RuntimeInvalid, message)
}

enum LiveMatch {
    Gone,
    Mismatch,
    IdentityUnavailable,
    Verified(Option<std::path::PathBuf>),
}

fn live_match(lease: &ChildLease, expected_digest: &str) -> LiveMatch {
    let Some(raw) = Pid::from_raw(lease.pid) else {
        return LiveMatch::Gone;
    };
    match test_kill_process(raw) {
        Err(err) if err == rustix::io::Errno::SRCH => return LiveMatch::Gone,
        Ok(()) => {}
        Err(_) => return LiveMatch::IdentityUnavailable,
    }
    let Some(live_pgid) = live_pgid(lease.pid) else {
        return LiveMatch::IdentityUnavailable;
    };
    if live_pgid != lease.pgid || live_pgid != lease.pid {
        return LiveMatch::Mismatch;
    }
    match read_start_key(lease.pid) {
        None => LiveMatch::IdentityUnavailable,
        Some(key) if key != lease.start_key => LiveMatch::Mismatch,
        Some(_) => match live_executable(lease.pid) {
            None => LiveMatch::IdentityUnavailable,
            Some((digest, _)) if digest != expected_digest || digest != lease.binary_sha256 => {
                LiveMatch::Mismatch
            }
            Some((_, staged_executable)) => {
                let leased = lease.staged_executable.as_deref().map(Path::new);
                if leased.is_some() && leased != staged_executable.as_deref() {
                    LiveMatch::Mismatch
                } else {
                    LiveMatch::Verified(staged_executable.or_else(|| leased.map(Path::to_path_buf)))
                }
            }
        },
    }
}

fn live_pgid(pid: i32) -> Option<i32> {
    let raw = Pid::from_raw(pid)?;
    getpgid(Some(raw)).ok().map(|g| g.as_raw_nonzero().get())
}

fn signal_verified_group(pgid: i32) -> Result<(), PlatformError> {
    let Some(raw) = Pid::from_raw(pgid) else {
        return Err(PlatformError::new(
            ErrorCode::RuntimeInvalid,
            "child lease process group is invalid",
        ));
    };
    crate::process::record_kill_target(pgid);
    match kill_process_group(raw, Signal::KILL) {
        Ok(()) => Ok(()),
        Err(err) if err == rustix::io::Errno::SRCH => Ok(()),
        Err(_) => Err(recovery_refused(
            "failed to signal verified orphaned runtime process group",
        )),
    }
}

fn wait_leader_and_group(pid: i32, pgid: i32) -> Result<(), PlatformError> {
    if pid != pgid {
        return Err(recovery_refused(
            "orphaned runtime is not its process group leader",
        ));
    }
    let raw =
        Pid::from_raw(pid).ok_or_else(|| recovery_refused("orphaned runtime PID is invalid"))?;
    let started = std::time::Instant::now();
    loop {
        // Usually the orphan is reparented, but unit tests and a fast restart
        // can still make this process its parent. Reap it when permitted.
        let _ = waitid(
            WaitId::Pid(raw),
            WaitIdOptions::NOHANG | WaitIdOptions::EXITED,
        );
        if assert_reaped(Some(pid)).is_ok() {
            return Ok(());
        }
        if started.elapsed() >= REAP_DEADLINE {
            return Err(recovery_refused(
                "orphaned runtime process group was not reaped",
            ));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn load_lease(path: &Path) -> Result<Option<ChildLease>, PlatformError> {
    let Some(mut file) = open_optional_nofollow(path)? else {
        return Ok(None);
    };
    let meta = file
        .metadata()
        .map_err(|_| PlatformError::new(ErrorCode::PathInvalid, "failed to stat child lease"))?;
    if !meta.file_type().is_file() || meta.len() > MAX_LEASE_BYTES {
        return Err(recovery_refused(
            "child lease is not a bounded regular file",
        ));
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|_| PlatformError::new(ErrorCode::PathInvalid, "failed to read child lease"))?;
    serde_json::from_slice::<ChildLease>(&bytes)
        .map(Some)
        .map_err(|_| recovery_refused("child lease is malformed"))
}

fn read_start_key(pid: i32) -> Option<String> {
    #[cfg(any(test, feature = "test-support"))]
    if let Some(hook) = *START_KEY_HOOK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
    {
        return hook(pid);
    }
    os_start_key(pid)
}

fn os_start_key(pid: i32) -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        linux_start_key(pid)
    }
    #[cfg(target_os = "macos")]
    {
        macos_ps_lstart(pid)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = pid;
        None
    }
}

#[cfg(target_os = "linux")]
fn linux_start_key(pid: i32) -> Option<String> {
    let text = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let rparen = text.rfind(')')?;
    let rest = text.get(rparen + 2..)?;
    let start = rest.split_whitespace().nth(19)?;
    Some(format!("linux:{start}"))
}

#[cfg(target_os = "macos")]
fn macos_ps_lstart(pid: i32) -> Option<String> {
    let out = Command::new("/bin/ps")
        .args(["-p", &pid.to_string(), "-o", "lstart="])
        .stdin(Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8(out.stdout).ok()?;
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(format!("darwin:{trimmed}"))
}

#[cfg(test)]
fn live_executable_digest(pid: i32) -> Option<String> {
    live_executable(pid).map(|(digest, _)| digest)
}

fn live_executable(pid: i32) -> Option<(String, Option<std::path::PathBuf>)> {
    #[cfg(target_os = "linux")]
    {
        linux_exe_digest(pid).map(|digest| (digest, None))
    }
    #[cfg(target_os = "macos")]
    {
        macos_txt_executable(pid)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = pid;
        None
    }
}

fn staged_executable_path(pid: i32) -> Option<std::path::PathBuf> {
    #[cfg(target_os = "macos")]
    {
        macos_txt_path(pid)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = pid;
        None
    }
}

#[cfg(target_os = "linux")]
fn linux_exe_digest(pid: i32) -> Option<String> {
    let mut file = std::fs::File::open(format!("/proc/{pid}/exe")).ok()?;
    let digest = hash_file(&mut file).ok()?;
    Some(hex_sha256(&digest))
}

#[cfg(target_os = "macos")]
fn macos_txt_path(pid: i32) -> Option<std::path::PathBuf> {
    let out = Command::new("lsof")
        .args(["-p", &pid.to_string(), "-a", "-d", "txt", "-Fn"])
        .stdin(Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8(out.stdout).ok()?;
    let path = text
        .lines()
        .find_map(|line| line.strip_prefix('n'))
        .map(std::path::PathBuf::from)?;
    path.canonicalize().ok()
}

#[cfg(target_os = "macos")]
fn macos_txt_executable(pid: i32) -> Option<(String, Option<std::path::PathBuf>)> {
    let path = macos_txt_path(pid)?;
    let mut file = open_nofollow(&path, false, false).ok()?;
    let digest = hash_file(&mut file).ok()?;
    let staged = private_staging_path(&path).then_some(path);
    Some((hex_sha256(&digest), staged))
}

fn private_staging_path(path: &Path) -> bool {
    let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    let Some(dir) = path.parent() else {
        return false;
    };
    let Some(dir_name) = dir.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    let Some(uuid) = dir_name.strip_prefix("oc-exec-") else {
        return false;
    };
    if file_name != "workerd" || uuid::Uuid::parse_str(uuid).is_err() {
        return false;
    }
    let Some(parent) = dir.parent() else {
        return false;
    };
    match (parent.canonicalize(), std::env::temp_dir().canonicalize()) {
        (Ok(parent), Ok(temp)) => parent == temp,
        _ => false,
    }
}

fn cleanup_dead_staging(lease: &ChildLease, expected_digest: &str) -> Result<(), PlatformError> {
    if lease.schema_version == SCHEMA
        && lease.binary_sha256 == expected_digest
        && let Some(path) = lease.staged_executable.as_deref()
    {
        cleanup_staging(Path::new(path), expected_digest)?;
    }
    Ok(())
}

fn cleanup_staging(path: &Path, expected_digest: &str) -> Result<(), PlatformError> {
    #[cfg(target_os = "macos")]
    {
        let dir = path
            .parent()
            .ok_or_else(|| recovery_refused("child lease staging path has no parent"))?;
        if !path.exists() && !dir.exists() {
            return Ok(());
        }
        if !private_staging_path(path) {
            return Err(recovery_refused(
                "child lease staging path is outside the private runtime staging root",
            ));
        }
        if !path.exists() {
            return match std::fs::remove_dir(dir) {
                Ok(()) => Ok(()),
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(_) => Err(recovery_refused(
                    "failed to remove empty runtime staging directory",
                )),
            };
        }
        let mut file = open_nofollow(path, false, false).map_err(|_| {
            recovery_refused("failed to reopen verified runtime staging executable")
        })?;
        let digest = hash_file(&mut file)
            .map_err(|_| recovery_refused("failed to hash verified runtime staging executable"))?;
        if hex_sha256(&digest) != expected_digest {
            return Err(recovery_refused(
                "runtime staging executable no longer matches its lease",
            ));
        }
        drop(file);
        std::fs::remove_file(path)
            .map_err(|_| recovery_refused("failed to remove runtime staging executable"))?;
        std::fs::remove_dir(dir)
            .map_err(|_| recovery_refused("failed to remove runtime staging directory"))?;
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (path, expected_digest);
        Err(recovery_refused(
            "runtime staging paths are unsupported on this operating system",
        ))
    }
}

#[cfg(test)]
#[path = "lease_tests.rs"]
mod tests;
