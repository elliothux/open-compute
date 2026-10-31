//! The only production runtime source: this executable's pinned, offline payload.

use crate::compile::cleanup_interrupted_compile_state;
use crate::fsutil::{
    StagingDir, create_dir_secure, fsync_dir, open_dir, open_file, rename_noreplace,
    write_atomic_new,
};
use crate::{RuntimeLock, VerifiedLaunchImage, VerifiedRuntime};
use flate2::read::GzDecoder;
use open_compute_core::{
    CompatibilityFeatureV1, CompatibilityImplicationV1, ErrorCode, PlatformError, Redactor,
    RuntimeCompatibilityV1, SystemWorkerCompatibilityV1,
};
use rustix::fs::{Mode, fchmod};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

mod payload {
    include!(concat!(env!("OUT_DIR"), "/embedded_payload.rs"));
}

mod cache_clean;
pub use cache_clean::{RuntimeCacheCleanReport, clean_embedded_runtime_cache};

const MAX_BINARY_BYTES: u64 = 256 * 1024 * 1024;
const MAX_PYODIDE_BUNDLE_BYTES: u64 = 32 * 1024 * 1024;

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

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct WorkerdCatalog {
    schema_version: u32,
    validation: String,
    binary_maximum_date: String,
    future_dates_allowed: bool,
    features: Vec<WorkerdCatalogFeature>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct WorkerdCatalogFeature {
    field: String,
    enable_flag: Option<String>,
    disable_flag: Option<String>,
    default_on_date: Option<String>,
    enabled_for_all_dates: bool,
    experimental: bool,
    python_snapshot_release: bool,
    #[serde(default)]
    implied_by: Vec<WorkerdCatalogImplication>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct WorkerdCatalogImplication {
    flags: Vec<String>,
    after_date: String,
}

/// Return tenant and system-Worker compatibility from the embedded formal runtime payload.
pub fn embedded_runtime_compatibility()
-> Result<(RuntimeCompatibilityV1, SystemWorkerCompatibilityV1), PlatformError> {
    let (lock, _) = embedded_runtime_lock()?;
    let bytes = payload::FILES
        .iter()
        .find_map(|(name, bytes)| (*name == "runtime/compatibility-catalog.json").then_some(*bytes))
        .ok_or_else(|| invalid("embedded compatibility catalog is missing"))?;
    if hex::encode(Sha256::digest(bytes)) != lock.compatibility_catalog.sha256 {
        return Err(invalid(
            "embedded compatibility catalog does not match its formal pin",
        ));
    }
    let catalog: WorkerdCatalog = serde_json::from_slice(bytes)
        .map_err(|_| invalid("embedded compatibility catalog is invalid"))?;
    if catalog.schema_version != lock.compatibility_catalog.schema_version
        || catalog.validation != "code_version"
        || catalog.binary_maximum_date != lock.binary_maximum_compatibility_date
        || catalog.future_dates_allowed
    {
        return Err(invalid(
            "embedded compatibility catalog does not match the runtime lock",
        ));
    }
    let compatibility = RuntimeCompatibilityV1 {
        validation: "workerd_code_version".to_owned(),
        binary_maximum_date: catalog.binary_maximum_date,
        future_dates_allowed: catalog.future_dates_allowed,
        experimental_enabled: lock
            .process_flags
            .iter()
            .any(|flag| flag == "--experimental"),
        features: catalog
            .features
            .into_iter()
            .map(|feature| CompatibilityFeatureV1 {
                field: feature.field,
                enable_flag: feature.enable_flag,
                disable_flag: feature.disable_flag,
                default_on_date: feature.default_on_date,
                enabled_for_all_dates: feature.enabled_for_all_dates,
                experimental: feature.experimental,
                python_snapshot_release: feature.python_snapshot_release,
                implied_by: feature
                    .implied_by
                    .into_iter()
                    .map(|rule| CompatibilityImplicationV1 {
                        flags: rule.flags,
                        after_date: rule.after_date,
                    })
                    .collect(),
            })
            .collect(),
        catalog_sha256: lock.compatibility_catalog.sha256,
    };
    let system_workers = SystemWorkerCompatibilityV1 {
        compatibility_date: lock.system_compatibility_date,
        compatibility_flags: lock.system_compatibility_flags,
    };
    if !compatibility.validate() {
        return Err(invalid("embedded compatibility catalog is invalid"));
    }
    Ok((compatibility, system_workers))
}

/// Return the formally pinned Caddy lock embedded in this executable.
pub fn embedded_caddy_lock() -> Result<&'static [u8], PlatformError> {
    payload::FILES
        .iter()
        .find_map(|(name, bytes)| (*name == "runtime/caddy.lock.json").then_some(*bytes))
        .ok_or_else(|| invalid("embedded Caddy lock is missing"))
}

/// Deterministic identity of the embedded template and generated system Workers.
#[must_use]
pub const fn embedded_runtime_assets_sha256() -> &'static str {
    payload::ASSETS_SHA256
}

/// Verified, embedded Browser Run viewer, built from the maintained TypeScript source.
#[must_use]
pub fn embedded_browser_view() -> Option<&'static [u8]> {
    payload::FILES
        .iter()
        .find_map(|(name, bytes)| (*name == "runtime/dist/browser/live-view.js").then_some(*bytes))
}

/// Content identity of the complete target-specific embedded runtime package.
#[must_use]
pub const fn embedded_payload_sha256() -> &'static str {
    payload::PAYLOAD_SHA256
}

/// Verified, privately materialized files belonging to the embedded payload.
#[derive(Clone, Debug)]
pub struct RuntimePackage {
    root: PathBuf,
}

impl RuntimePackage {
    /// Open the packaged Caddy executable.
    pub fn caddy(&self) -> Result<VerifiedLaunchImage, PlatformError> {
        let file = open_file(&self.root.join("caddy"), false, false)?;
        Ok(VerifiedLaunchImage::from_verified_file(file))
    }

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

    /// Absolute private directory containing the verified Pyodide bundle.
    #[must_use]
    pub fn pyodide_bundle_cache_dir(&self) -> PathBuf {
        self.root.join("pyodide-bundle-cache")
    }

    /// Check the packaged executable version and recover orphaned process groups.
    pub async fn verify(
        &self,
        deadline: Duration,
        redactor: &Redactor,
        lease_path: &Path,
    ) -> Result<VerifiedRuntime, PlatformError> {
        embedded_runtime_compatibility()?;
        let pyodide_bundle_cache = self.pyodide_bundle_cache_dir();
        crate::verify::verify_runtime_binary_inner(
            embedded_runtime_lock()?.1,
            &self.root.join("workerd"),
            deadline,
            redactor,
            Some(lease_path),
            Some(&pyodide_bundle_cache),
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
    let _ = open_dir(runtime_dir)?;
    cleanup_interrupted_compile_state(runtime_dir)?;
    let packages = runtime_dir.join("packages");
    create_dir_secure(&packages)?;
    cleanup_partial_packages(&packages)?;
    let root = packages.join(payload::PAYLOAD_SHA256);
    match std::fs::symlink_metadata(&root) {
        Ok(_) => {
            return Ok(RuntimePackage { root });
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err(invalid("embedded runtime package is inaccessible")),
    }
    let (lock, _) = embedded_runtime_lock()?;
    let mut staging = StagingDir::create(&packages, ".partial-runtime")?;
    unpack_payload(
        payload::ARCHIVE,
        &staging.path().join("workerd"),
        MAX_BINARY_BYTES,
        Mode::RUSR | Mode::XUSR,
    )?;
    write_payload(
        &staging.path().join("caddy"),
        payload::CADDY,
        Mode::RUSR | Mode::XUSR,
    )?;
    let mut directories = BTreeSet::new();
    let pyodide_cache = staging.path().join("pyodide-bundle-cache");
    create_dir_secure(&pyodide_cache)?;
    directories.insert(pyodide_cache.clone());
    unpack_payload(
        payload::PYODIDE_ARCHIVE,
        &pyodide_cache.join(&lock.pyodide_bundle.file_name),
        MAX_PYODIDE_BUNDLE_BYTES,
        Mode::RUSR,
    )?;
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
    rename_noreplace(staging.path(), &root)?;
    staging.persist();
    fsync_dir(&packages)?;
    fsync_dir(runtime_dir)?;
    Ok(RuntimePackage { root })
}

/// Check whether the current package is materialized; absence is not an error.
///
/// This never creates directories, runs workerd, or repairs a corrupt cache entry.
pub fn inspect_embedded_runtime(runtime_dir: &Path) -> Result<bool, PlatformError> {
    let root = runtime_dir.join("packages").join(payload::PAYLOAD_SHA256);
    match std::fs::symlink_metadata(&root) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(_) => Err(invalid("embedded runtime package is inaccessible")),
    }
}

/// Open an existing embedded package without mutating its cache.
///
/// This supports read-only operator tools while the daemon owns the data-directory lock.
pub fn open_materialized_runtime(runtime_dir: &Path) -> Result<RuntimePackage, PlatformError> {
    let root = runtime_dir.join("packages").join(payload::PAYLOAD_SHA256);
    let _ = open_dir(&root)?;
    Ok(RuntimePackage { root })
}

fn cleanup_partial_packages(packages: &Path) -> Result<(), PlatformError> {
    let directory = open_dir(packages)?;
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
        let _ = open_dir(&path)?;
        // The exclusive platform lock excludes a live materializer. remove_dir_all never
        // traverses symlinks, and only this private, UUID-tagged unpublished tree is removed.
        std::fs::remove_dir_all(&path)
            .map_err(|_| invalid("failed to recover interrupted runtime materialization"))?;
    }
    fsync_dir(packages)
}

fn unpack_payload(
    archive: &[u8],
    path: &Path,
    max_bytes: u64,
    mode: Mode,
) -> Result<(), PlatformError> {
    let mut decoder = GzDecoder::new(archive).take(max_bytes + 1);
    let mut file = open_file(path, true, true)?;
    let mut total = 0u64;
    let mut chunk = [0u8; 65536];
    loop {
        let count = decoder
            .read(&mut chunk)
            .map_err(|_| invalid("embedded payload archive is invalid"))?;
        if count == 0 {
            break;
        }
        total += count as u64;
        if total > max_bytes {
            return Err(invalid(
                "embedded payload exceeds its decompressed size bound",
            ));
        }
        file.write_all(&chunk[..count])
            .map_err(|_| invalid("failed to materialize embedded payload"))?;
    }
    fchmod(&file, mode).map_err(|_| invalid("failed to secure embedded payload"))?;
    file.sync_all()
        .map_err(|_| invalid("failed to sync embedded payload"))?;
    Ok(())
}

fn write_payload(path: &Path, bytes: &[u8], mode: Mode) -> Result<(), PlatformError> {
    let mut file = open_file(path, true, true)?;
    file.write_all(bytes)
        .map_err(|_| invalid("failed to materialize embedded payload"))?;
    fchmod(&file, mode).map_err(|_| invalid("failed to secure embedded payload"))?;
    file.sync_all()
        .map_err(|_| invalid("failed to sync embedded payload"))
}

fn invalid(message: &'static str) -> PlatformError {
    PlatformError::new(ErrorCode::RuntimeInvalid, message)
}

#[cfg(test)]
#[path = "embedded_tests.rs"]
mod tests;
