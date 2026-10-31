//! Recorded child process groups for next-start orphan recovery.

use crate::fsutil::{open_optional_file, require_absolute, write_atomic_replace};
use crate::process::recover_unleased_staging;
use open_compute_core::{ErrorCode, PlatformError};
use rustix::process::{
    Pid, Signal, WaitId, WaitIdOptions, kill_process_group, test_kill_process_group, waitid,
};
use serde::{Deserialize, Serialize};
use std::io::Read;
use std::path::Path;
use std::time::{Duration, Instant};

const REAP_DEADLINE: Duration = Duration::from_secs(5);
const MAX_LEASE_BYTES: u64 = 4096;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
struct ChildLease {
    pgid: i32,
    // PGIDs are reusable after reboot; recovery must stay within one boot.
    #[serde(default)]
    boot_id: String,
}

#[cfg(any(test, feature = "test-support"))]
static WRITE_FAIL: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Test-only injected `write_lease` failure.
#[cfg(any(test, feature = "test-support"))]
pub fn set_lease_write_fail(fail: bool) {
    WRITE_FAIL.store(fail, std::sync::atomic::Ordering::SeqCst);
}

pub(crate) fn write_lease(path: &Path, pgid: i32) -> Result<(), PlatformError> {
    #[cfg(any(test, feature = "test-support"))]
    if WRITE_FAIL.load(std::sync::atomic::Ordering::SeqCst) {
        return Err(recovery_failed("injected lease write failure"));
    }
    require_absolute(path)?;
    let lease = ChildLease {
        pgid,
        boot_id: host_boot_id()?,
    };
    let bytes =
        serde_json::to_vec(&lease).map_err(|_| recovery_failed("failed to encode child lease"))?;
    write_atomic_replace(path, &bytes, 0o600)
}

pub(crate) fn clear_lease(path: &Path) -> Result<(), PlatformError> {
    require_absolute(path)?;
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(recovery_failed("failed to remove child lease")),
    }
}

/// Stop the recorded process group, reap when permitted, then clear recovery state.
pub(crate) fn recover_orphans(path: &Path) -> Result<Option<i32>, PlatformError> {
    require_absolute(path)?;
    let lease = load_lease(path)?;
    if let Some(lease) = &lease {
        let raw = group_pid(lease)?;
        let started = Instant::now();
        signal_group(raw, Signal::TERM)?;
        loop {
            // A fast restart or a test may still own the child. Reap it when permitted.
            let _ = waitid(
                WaitId::Pid(raw),
                WaitIdOptions::NOHANG | WaitIdOptions::EXITED,
            );
            match test_kill_process_group(raw) {
                Err(error) if error == rustix::io::Errno::SRCH => break,
                Err(_) => return Err(recovery_failed("failed to inspect orphan process group")),
                Ok(()) => {}
            }
            if started.elapsed() >= REAP_DEADLINE {
                return Err(recovery_failed(
                    "orphan process group did not exit before deadline",
                ));
            }
            if started.elapsed() >= Duration::from_millis(200) {
                signal_group(raw, Signal::KILL)?;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    recover_unleased_staging(path)?;
    clear_lease(path)?;
    Ok(lease.map(|lease| lease.pgid))
}

/// Refuse offline mutation while the recorded runtime process group is still live.
pub fn assert_no_live_orphan(path: &Path) -> Result<(), PlatformError> {
    require_absolute(path)?;
    let Some(lease) = load_lease(path)? else {
        return Ok(());
    };
    match test_kill_process_group(group_pid(&lease)?) {
        Err(error) if error == rustix::io::Errno::SRCH => Ok(()),
        Err(_) => Err(recovery_failed("failed to inspect runtime process group")),
        Ok(()) => Err(PlatformError::new(
            ErrorCode::PlatformUnavailable,
            "workerd process group is still live; offline operation refused",
        )),
    }
}

/// Recover a recorded process group from a process-level integration test.
#[cfg(any(test, feature = "test-support"))]
pub fn recover_orphan_for_test(path: &Path) -> Result<Option<i32>, PlatformError> {
    recover_orphans(path)
}

fn group_pid(lease: &ChildLease) -> Result<Pid, PlatformError> {
    if lease.pgid <= 1 {
        return Err(recovery_failed("child lease process group is invalid"));
    }
    Pid::from_raw(lease.pgid).ok_or_else(|| recovery_failed("child lease process group is invalid"))
}

fn signal_group(pgid: Pid, signal: Signal) -> Result<(), PlatformError> {
    match signal {
        Signal::KILL => crate::process::record_kill_target(pgid.as_raw_nonzero().get()),
        _ => crate::process::record_signal(pgid.as_raw_nonzero().get(), "TERM"),
    }
    match kill_process_group(pgid, signal) {
        Ok(()) => Ok(()),
        Err(error) if error == rustix::io::Errno::SRCH => Ok(()),
        Err(_) => Err(recovery_failed("failed to signal orphan process group")),
    }
}

fn load_lease(path: &Path) -> Result<Option<ChildLease>, PlatformError> {
    let Some(file) = open_optional_file(path)? else {
        return Ok(None);
    };
    let mut bytes = Vec::new();
    file.take(MAX_LEASE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| recovery_failed("failed to read child lease"))?;
    if bytes.len() as u64 > MAX_LEASE_BYTES {
        return Err(recovery_failed("child lease exceeds its size bound"));
    }
    let lease: ChildLease =
        serde_json::from_slice(&bytes).map_err(|_| recovery_failed("child lease is malformed"))?;
    if lease.boot_id.is_empty() || lease.boot_id != host_boot_id()? {
        return Ok(None);
    }
    Ok(Some(lease))
}

fn host_boot_id() -> Result<String, PlatformError> {
    #[cfg(target_os = "linux")]
    let boot_id = std::fs::read_to_string("/proc/sys/kernel/random/boot_id")
        .map_err(|_| recovery_failed("failed to read host boot identity"))?;
    #[cfg(target_os = "macos")]
    let boot_id = {
        let output = std::process::Command::new("/usr/sbin/sysctl")
            .args(["-n", "kern.boottime"])
            .output()
            .map_err(|_| recovery_failed("failed to read host boot identity"))?;
        if !output.status.success() {
            return Err(recovery_failed("failed to read host boot identity"));
        }
        String::from_utf8(output.stdout)
            .map_err(|_| recovery_failed("host boot identity is not UTF-8"))?
    };
    let boot_id = boot_id.trim().to_owned();
    if boot_id.is_empty() {
        return Err(recovery_failed("host boot identity is empty"));
    }
    Ok(boot_id)
}

fn recovery_failed(message: &'static str) -> PlatformError {
    PlatformError::new(ErrorCode::RuntimeInvalid, message)
}

#[cfg(test)]
#[path = "lease_tests.rs"]
mod tests;
