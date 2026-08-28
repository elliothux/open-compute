//! The only production runtime source: this executable's pinned, offline payload.

use crate::fsutil::{
    StagingDir, create_dir_secure, fsync_dir, hash_bytes, hash_file, open_dir_nofollow,
    open_nofollow, parse_sha256_hex, read_regular_nofollow, rename_noreplace,
    require_executable_fd, write_atomic_new,
};
use crate::{RuntimeLock, VerifiedRuntime, runtime_assets_sha256};
use flate2::read::GzDecoder;
use open_compute_core::{ErrorCode, PlatformError, Redactor};
use rustix::fs::{Mode, fchmod};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

mod payload {
    include!(concat!(env!("OUT_DIR"), "/embedded_payload.rs"));
}

const MAX_BINARY_BYTES: u64 = 256 * 1024 * 1024;

/// Parse the formal lock compiled into this executable, without filesystem access.
pub fn embedded_runtime_lock() -> Result<(RuntimeLock, &'static [u8]), PlatformError> {
    let bytes = payload::FILES
        .iter()
        .find_map(|(name, bytes)| (*name == "runtime/workerd.lock.json").then_some(*bytes))
        .ok_or_else(|| invalid("embedded runtime lock is missing"))?;
    let lock = RuntimeLock::parse(bytes)?;
    if lock.current_target()?.0 != payload::TARGET {
        return Err(invalid(
            "embedded runtime target does not match this executable",
        ));
    }
    Ok((lock, bytes))
}

/// Deterministic identity of the embedded template and generated system Workers.
#[must_use]
pub const fn embedded_runtime_assets_sha256() -> &'static str {
    payload::ASSETS_SHA256
}

/// Content identity of the complete target-specific embedded runtime package.
#[must_use]
pub const fn embedded_payload_sha256() -> &'static str {
    payload::PAYLOAD_SHA256
}

/// Verified, privately materialized files belonging to the embedded payload.
#[derive(Debug)]
pub struct RuntimePackage {
    root: PathBuf,
}

impl RuntimePackage {
    /// Absolute embedded lock path used by the static configuration compiler.
    #[must_use]
    pub fn lock_path(&self) -> PathBuf {
        self.root.join("runtime/workerd.lock.json")
    }

    /// Absolute embedded template and generated Worker directory.
    #[must_use]
    pub fn assets_dir(&self) -> PathBuf {
        self.root.join("runtime")
    }

    /// Verify the pinned executable and its version, recovering only authenticated orphans.
    pub async fn verify(
        &self,
        deadline: Duration,
        redactor: &Redactor,
        lease_path: &Path,
    ) -> Result<VerifiedRuntime, PlatformError> {
        crate::verify::verify_runtime_binary_inner(
            embedded_runtime_lock()?.1,
            &self.root.join("workerd"),
            deadline,
            redactor,
            Some(lease_path),
            Some(payload::ASSETS_SHA256),
        )
        .await
    }
}

/// Materialize the embedded payload under an exclusively owned data directory.
///
/// The caller must hold the platform data-directory lock throughout materialization and use.
/// No network, PATH lookup, configuration override, or replacement of corrupt packages exists.
/// Staging is private, same-filesystem, fsynced, and atomically published without overwrite.
pub fn materialize_embedded_runtime(runtime_dir: &Path) -> Result<RuntimePackage, PlatformError> {
    let _ = open_dir_nofollow(runtime_dir)?;
    let packages = runtime_dir.join("packages");
    create_dir_secure(&packages)?;
    cleanup_partial_packages(&packages)?;
    let root = packages.join(payload::PAYLOAD_SHA256);
    match std::fs::symlink_metadata(&root) {
        Ok(_) => {
            verify_package(&root)?;
            return Ok(RuntimePackage { root });
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err(invalid("embedded runtime package is inaccessible")),
    }
    let (lock, _) = embedded_runtime_lock()?;
    let (_, target) = lock.current_target()?;
    if hash_bytes(payload::ARCHIVE) != parse_sha256_hex(&target.archive_sha256)? {
        return Err(invalid(
            "embedded workerd archive does not match its formal pin",
        ));
    }
    let mut staging = StagingDir::create(&packages, ".partial-runtime")?;
    unpack_binary(
        payload::ARCHIVE,
        &staging.path().join("workerd"),
        &target.binary_sha256,
    )?;
    let mut directories = BTreeSet::new();
    for (name, bytes) in payload::FILES {
        let path = staging.path().join(name);
        let parent = path
            .parent()
            .ok_or_else(|| invalid("embedded asset has no parent"))?;
        let ancestors: Vec<_> = parent
            .ancestors()
            .take_while(|path| path.starts_with(staging.path()))
            .collect();
        for ancestor in ancestors.into_iter().rev() {
            create_dir_secure(ancestor)?;
            directories.insert(ancestor.to_owned());
        }
        write_atomic_new(&path, bytes, 0o400)?;
    }
    for directory in directories.iter().rev() {
        fsync_dir(directory)?;
    }
    verify_package(staging.path())?;
    rename_noreplace(staging.path(), &root)?;
    staging.persist();
    fsync_dir(&packages)?;
    fsync_dir(runtime_dir)?;
    Ok(RuntimePackage { root })
}

/// Read-only validation of a previously materialized package; absence is not an error.
///
/// This never creates directories, runs workerd, or repairs a corrupt cache entry.
pub fn inspect_embedded_runtime(runtime_dir: &Path) -> Result<bool, PlatformError> {
    let root = runtime_dir.join("packages").join(payload::PAYLOAD_SHA256);
    match std::fs::symlink_metadata(&root) {
        Ok(_) => {
            verify_package(&root)?;
            Ok(true)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(_) => Err(invalid("embedded runtime package is inaccessible")),
    }
}

fn verify_package(root: &Path) -> Result<(), PlatformError> {
    let _ = open_dir_nofollow(root)?;
    let (lock, _) = embedded_runtime_lock()?;
    let (_, target) = lock.current_target()?;
    let mut file = open_nofollow(&root.join("workerd"), false, false)?;
    if require_executable_fd(&file)?.len() > MAX_BINARY_BYTES
        || hash_file(&mut file)? != parse_sha256_hex(&target.binary_sha256)?
    {
        return Err(invalid(
            "materialized workerd does not match the embedded pin",
        ));
    }
    for (name, bytes) in payload::FILES {
        if read_regular_nofollow(&root.join(name))? != *bytes {
            return Err(invalid(
                "materialized runtime asset does not match the embedded payload",
            ));
        }
    }
    if runtime_assets_sha256(&root.join("runtime"))? != payload::ASSETS_SHA256 {
        return Err(invalid(
            "materialized system Worker set does not match the embedded payload",
        ));
    }
    Ok(())
}

fn cleanup_partial_packages(packages: &Path) -> Result<(), PlatformError> {
    let directory = open_dir_nofollow(packages)?;
    let entries = rustix::fs::Dir::read_from(&directory)
        .map_err(|_| invalid("failed to inspect runtime package staging"))?;
    for (count, entry) in entries.enumerate() {
        if count > 4096 {
            return Err(invalid("runtime package directory exceeds its entry bound"));
        }
        let entry = entry.map_err(|_| invalid("failed to inspect runtime package staging"))?;
        let Ok(name) = entry.file_name().to_str() else {
            continue;
        };
        let Some(id) = name.strip_prefix(".partial-runtime-") else {
            continue;
        };
        if !uuid::Uuid::parse_str(id).is_ok_and(|id| id.get_version_num() == 7) {
            return Err(invalid("runtime staging directory has an invalid identity"));
        }
        let path = packages.join(name);
        let _ = open_dir_nofollow(&path)?;
        // The exclusive platform lock excludes a live materializer. remove_dir_all never
        // traverses symlinks, and only this private, UUID-tagged unpublished tree is removed.
        std::fs::remove_dir_all(&path)
            .map_err(|_| invalid("failed to recover interrupted runtime materialization"))?;
    }
    fsync_dir(packages)
}

fn unpack_binary(archive: &[u8], path: &Path, expected: &str) -> Result<(), PlatformError> {
    let mut decoder = GzDecoder::new(archive).take(MAX_BINARY_BYTES + 1);
    let mut file = open_nofollow(path, true, true)?;
    let mut hasher = Sha256::new();
    let mut total = 0u64;
    let mut chunk = [0u8; 65536];
    loop {
        let count = decoder
            .read(&mut chunk)
            .map_err(|_| invalid("embedded workerd archive is invalid"))?;
        if count == 0 {
            break;
        }
        total += count as u64;
        if total > MAX_BINARY_BYTES {
            return Err(invalid(
                "embedded workerd exceeds the decompressed size bound",
            ));
        }
        hasher.update(&chunk[..count]);
        file.write_all(&chunk[..count])
            .map_err(|_| invalid("failed to materialize embedded workerd"))?;
    }
    if hex::encode(hasher.finalize()) != expected {
        return Err(invalid(
            "decompressed workerd does not match its formal pin",
        ));
    }
    fchmod(&file, Mode::RUSR | Mode::XUSR)
        .map_err(|_| invalid("failed to secure embedded workerd"))?;
    file.sync_all()
        .map_err(|_| invalid("failed to sync embedded workerd"))?;
    Ok(())
}

fn invalid(message: &'static str) -> PlatformError {
    PlatformError::new(ErrorCode::RuntimeInvalid, message)
}

#[cfg(test)]
#[path = "embedded_tests.rs"]
mod tests;
