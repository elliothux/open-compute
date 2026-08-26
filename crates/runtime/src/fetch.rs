//! Operator-only official workerd archive install. Not used at production startup.

use crate::fsutil::{
    MAX_ARCHIVE_BYTES, MAX_ASSET_FILE_BYTES, StagingDir, create_dir_secure, fsync_dir, hash_bytes,
    open_dir_nofollow, open_nofollow, parse_sha256_hex, read_regular_nofollow_bounded,
    reject_symlink_escape, rename_noreplace, require_absolute, write_atomic_new,
};
use crate::lock::{RuntimeLock, RuntimeTarget};
use crate::process::{assert_reaped, exec_image};
use flate2::read::GzDecoder;
use open_compute_core::{ErrorCode, PlatformError, Redactor};
use std::fs;
use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

const VERSION_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_WORKERD_BINARY_BYTES: usize = 256 * 1024 * 1024;
const _: () = assert!(MAX_WORKERD_BINARY_BYTES > MAX_ARCHIVE_BYTES);

fn checked_decompressed_len(current: usize, chunk: usize) -> Result<usize, PlatformError> {
    let next = current.checked_add(chunk).ok_or_else(|| {
        PlatformError::new(
            ErrorCode::RuntimeInvalid,
            "decompressed binary exceeds the size bound",
        )
    })?;
    if next > MAX_WORKERD_BINARY_BYTES {
        return Err(PlatformError::new(
            ErrorCode::RuntimeInvalid,
            "decompressed binary exceeds the size bound",
        ));
    }
    Ok(next)
}

/// Install the lock-selected official workerd binary into `dest_dir/bin/workerd`.
///
/// Downloads only when `download` is `true`. Production verification never calls this.
pub fn install_official_release(
    lock: &RuntimeLock,
    dest_dir: &Path,
    download: bool,
    archive_bytes: Option<&[u8]>,
) -> Result<(), PlatformError> {
    require_absolute(dest_dir)?;
    let _ = open_dir_nofollow(parent_of_dest(dest_dir)?)?;
    if dest_dir.exists() {
        return Err(PlatformError::new(
            ErrorCode::PathInvalid,
            "release destination already exists",
        ));
    }
    let (_target_name, target) = lock.current_target()?;
    let archive = match archive_bytes {
        Some(bytes) => {
            if bytes.len() > MAX_ARCHIVE_BYTES {
                return Err(PlatformError::new(
                    ErrorCode::RuntimeInvalid,
                    "archive exceeds the size bound",
                ));
            }
            bytes.to_vec()
        }
        None if download => download_official(target)?,
        None => {
            return Err(PlatformError::new(
                ErrorCode::RuntimeInvalid,
                "archive bytes are required when download is disabled",
            ));
        }
    };
    if archive.len() > MAX_ARCHIVE_BYTES {
        return Err(PlatformError::new(
            ErrorCode::RuntimeInvalid,
            "archive exceeds the size bound",
        ));
    }
    let expected_archive = parse_sha256_hex(&target.archive_sha256)?;
    if hash_bytes(&archive) != expected_archive {
        return Err(PlatformError::new(
            ErrorCode::RuntimeInvalid,
            "archive hash does not match the lock",
        ));
    }
    let mut decoder = GzDecoder::new(archive.as_slice());
    let mut binary = Vec::new();
    let mut buf = [0u8; 8192];
    loop {
        let n = decoder.read(&mut buf).map_err(|_| {
            PlatformError::new(
                ErrorCode::RuntimeInvalid,
                "failed to decompress workerd archive",
            )
        })?;
        if n == 0 {
            break;
        }
        let _ = checked_decompressed_len(binary.len(), n)?;
        binary.extend_from_slice(&buf[..n]);
    }
    let expected_binary = parse_sha256_hex(&target.binary_sha256)?;
    if hash_bytes(&binary) != expected_binary {
        return Err(PlatformError::new(
            ErrorCode::RuntimeInvalid,
            "decompressed binary hash does not match the lock",
        ));
    }

    let parent = parent_of_dest(dest_dir)?;
    let mut staging = StagingDir::create(parent, ".partial-release")?;
    let bin_dir = staging.path().join("bin");
    create_dir_secure(&bin_dir)?;
    let bin_path = bin_dir.join("workerd");
    write_atomic_new(&bin_path, &binary, 0o755)?;

    let file = open_nofollow(&bin_path, false, false)?;
    let image = exec_image(&file)?;
    let mut child = Command::new(&image.program)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()
        .map_err(|_| {
            PlatformError::new(
                ErrorCode::RuntimeInvalid,
                "failed to execute installed workerd --version",
            )
        })?;
    let pid = Some(child.id() as i32);
    let started = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if started.elapsed() > VERSION_TIMEOUT => {
                if let Some(raw) = pid
                    && let Some(p) = rustix::process::Pid::from_raw(raw)
                {
                    let _ = rustix::process::kill_process_group(p, rustix::process::Signal::KILL);
                }
                let _ = child.kill();
                let _ = child.wait();
                let _ = assert_reaped(pid);
                return Err(PlatformError::new(
                    ErrorCode::RuntimeInvalid,
                    "installed workerd --version timed out",
                ));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(_) => {
                return Err(PlatformError::new(
                    ErrorCode::RuntimeInvalid,
                    "failed to wait for installed workerd --version",
                ));
            }
        }
    }
    let version = child.wait_with_output().map_err(|_| {
        PlatformError::new(
            ErrorCode::RuntimeInvalid,
            "failed to execute installed workerd --version",
        )
    })?;
    assert_reaped(pid)?;
    if !version.status.success() {
        return Err(PlatformError::new(
            ErrorCode::RuntimeInvalid,
            "installed workerd --version exited unsuccessfully",
        ));
    }
    let stdout = std::str::from_utf8(&version.stdout).unwrap_or("");
    let _ = Redactor::new();
    if stdout.trim() != lock.expected_version_output {
        return Err(PlatformError::new(
            ErrorCode::RuntimeInvalid,
            "installed workerd version does not match the lock",
        ));
    }
    match rename_noreplace(staging.path(), dest_dir) {
        Ok(()) => {
            staging.persist();
            fsync_dir(parent)?;
            Ok(())
        }
        Err(err) => Err(err),
    }
}

/// Inputs for [`package_release_bundle`].
#[derive(Debug)]
pub struct PackageReleaseRequest<'a> {
    /// Parsed lock.
    pub lock: &'a RuntimeLock,
    /// Destination root. Must not exist.
    pub dest_dir: &'a Path,
    /// Absolute `platformd` binary to copy.
    pub platformd: &'a Path,
    /// Absolute runtime assets directory.
    pub assets_dir: &'a Path,
    /// Absolute license file.
    pub license_file: &'a Path,
    /// Absolute default config file.
    pub default_config: &'a Path,
    /// Absolute directory containing the complete P1 operator runbook set.
    pub runbooks_dir: &'a Path,
    /// Canonical machine-readable P1 release compatibility metadata.
    pub release_json: &'a [u8],
    /// When true, fetch the official archive over HTTPS.
    pub download: bool,
    /// Optional already-read official archive (still hash-verified).
    pub archive_bytes: Option<&'a [u8]>,
}

/// Build the documented offline release layout. Fetches the official archive
/// only when `download` is true. Refuses an existing destination and any
/// checksum or version mismatch.
pub fn package_release_bundle(req: &PackageReleaseRequest<'_>) -> Result<(), PlatformError> {
    require_absolute(req.dest_dir)?;
    require_absolute(req.platformd)?;
    require_absolute(req.assets_dir)?;
    require_absolute(req.license_file)?;
    require_absolute(req.default_config)?;
    require_absolute(req.runbooks_dir)?;
    if req.dest_dir.exists() {
        return Err(PlatformError::new(
            ErrorCode::PathInvalid,
            "release destination already exists",
        ));
    }
    if req.release_json.is_empty()
        || req.release_json.len() > 1024 * 1024
        || !serde_json::from_slice::<serde_json::Value>(req.release_json)
            .is_ok_and(|value| value.is_object())
    {
        return Err(PlatformError::new(
            ErrorCode::ReleaseUnsupported,
            "release metadata is invalid",
        ));
    }
    let _ = req.lock.current_target()?;
    let parent = parent_of_dest(req.dest_dir)?;
    let mut staging = StagingDir::create(parent, ".partial-bundle")?;
    let inst = staging.path().join(".workerd-inst");
    install_official_release(req.lock, &inst, req.download, req.archive_bytes)?;
    let bin_dir = staging.path().join("bin");
    create_dir_secure(&bin_dir)?;
    rename_noreplace(&inst.join("bin").join("workerd"), &bin_dir.join("workerd"))?;
    fs::remove_dir_all(&inst).map_err(|_| {
        PlatformError::new(
            ErrorCode::PathInvalid,
            "failed to remove private workerd installer staging",
        )
    })?;
    copy_regular(req.platformd, &bin_dir.join("platformd"), 0o755)?;
    let runtime_dest = staging.path().join("runtime");
    create_dir_secure(&runtime_dest)?;
    copy_regular(
        &req.assets_dir.join("workerd.lock.json"),
        &runtime_dest.join("workerd.lock.json"),
        0o644,
    )?;
    copy_regular(
        &req.assets_dir.join("config.capnp"),
        &runtime_dest.join("config.capnp"),
        0o644,
    )?;
    copy_tree(
        req.assets_dir,
        &req.assets_dir.join("system-workers"),
        &runtime_dest.join("system-workers"),
    )?;
    let licenses = staging.path().join("licenses");
    create_dir_secure(&licenses)?;
    copy_regular(req.license_file, &licenses.join("LICENSE"), 0o644)?;
    let share = staging.path().join("share");
    create_dir_secure(&share)?;
    copy_regular(
        req.default_config,
        &share.join("default-config.toml"),
        0o644,
    )?;
    write_atomic_new(&share.join("release.json"), req.release_json, 0o644)?;
    let runbooks = staging.path().join("docs").join("runbooks");
    create_dir_secure(&staging.path().join("docs"))?;
    create_dir_secure(&runbooks)?;
    for name in [
        "install-and-first-start.md",
        "backup-and-retention.md",
        "fresh-host-restore.md",
        "upgrade-and-rollback.md",
        "disk-pressure.md",
        "sqlite-corruption.md",
        "s3-outage.md",
        "workerd-crash-loop.md",
        "master-key-loss-and-recovery.md",
        "scheduler-recovery.md",
        "collect-support-bundle.md",
    ] {
        copy_regular(&req.runbooks_dir.join(name), &runbooks.join(name), 0o644)?;
    }
    fsync_dir(staging.path())?;
    rename_noreplace(staging.path(), req.dest_dir)?;
    staging.persist();
    fsync_dir(parent)?;
    Ok(())
}

fn copy_regular(src: &Path, dest: &Path, mode: u32) -> Result<(), PlatformError> {
    require_absolute(src)?;
    require_absolute(dest)?;
    let bytes = read_regular_nofollow_bounded(src, MAX_ASSET_FILE_BYTES.max(64 * 1024 * 1024))?;
    write_atomic_new(dest, &bytes, mode)?;
    let _ = fs::set_permissions(dest, PermissionsExt::from_mode(mode));
    Ok(())
}

fn copy_tree(root: &Path, src: &Path, dest: &Path) -> Result<(), PlatformError> {
    reject_symlink_escape(root, src)?;
    create_dir_secure(dest)?;
    let entries = fs::read_dir(src).map_err(|_| {
        PlatformError::new(
            ErrorCode::PathInvalid,
            "failed to read package source directory",
        )
    })?;
    for entry in entries {
        let entry = entry.map_err(|_| {
            PlatformError::new(
                ErrorCode::PathInvalid,
                "failed to read package source directory",
            )
        })?;
        let name = entry.file_name();
        if name == "." || name == ".." {
            continue;
        }
        let child_src = src.join(&name);
        let child_dest = dest.join(&name);
        reject_symlink_escape(root, &child_src)?;
        let meta = fs::symlink_metadata(&child_src).map_err(|_| {
            PlatformError::new(ErrorCode::PathInvalid, "failed to read package source file")
        })?;
        if meta.file_type().is_symlink() {
            return Err(PlatformError::new(
                ErrorCode::PathInvalid,
                "package source must not be a symlink",
            ));
        }
        if meta.file_type().is_dir() {
            copy_tree(root, &child_src, &child_dest)?;
        } else if meta.file_type().is_file() {
            copy_regular(&child_src, &child_dest, 0o644)?;
        } else {
            return Err(PlatformError::new(
                ErrorCode::PathInvalid,
                "package source must contain only regular files and directories",
            ));
        }
    }
    Ok(())
}

fn parent_of_dest(dest_dir: &Path) -> Result<&Path, PlatformError> {
    dest_dir.parent().ok_or_else(|| {
        PlatformError::new(
            ErrorCode::PathInvalid,
            "release destination must have a parent",
        )
    })
}

fn download_official(target: &RuntimeTarget) -> Result<Vec<u8>, PlatformError> {
    // Operator-only: explicit argv, no shell interpolation, official URL from the lock.
    let output = Command::new("/usr/bin/curl")
        .args([
            "--fail",
            "--silent",
            "--show-error",
            "--location",
            "--proto",
            "=https",
            "--tlsv1.2",
            "--max-time",
            "60",
            "--max-filesize",
            &MAX_ARCHIVE_BYTES.to_string(),
            "--user-agent",
            "open-compute-release-fetch/0.1",
            "--output",
            "-",
            target.archive_url.as_str(),
        ])
        .output()
        .map_err(|_| {
            PlatformError::new(
                ErrorCode::RuntimeInvalid,
                "failed to invoke official archive download helper",
            )
        })?;
    if !output.status.success() {
        return Err(PlatformError::new(
            ErrorCode::RuntimeInvalid,
            "official archive download failed",
        ));
    }
    if output.stdout.len() > MAX_ARCHIVE_BYTES {
        return Err(PlatformError::new(
            ErrorCode::RuntimeInvalid,
            "archive exceeds the size bound",
        ));
    }
    Ok(output.stdout)
}

#[cfg(test)]
#[path = "fetch_tests.rs"]
mod size_tests;
