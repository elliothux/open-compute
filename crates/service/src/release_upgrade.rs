//! Formal release download, `ocd upgrade`, and `ocd uninstall`.

use crate::config_load::load_platform_config_from;
use crate::install_receipt::{
    self, InstallReceipt, cmp_stable_semver, is_stable_semver, path_looks_package_manager_owned,
    read_receipt, receipt_path_for_binary, require_upgradeable_receipt, write_receipt,
};
use crate::instance_ops::{INSTANCE_READY_TIMEOUT, wait_until_instance_ready_for_release};
use crate::instance_registry::InstanceRegistry;
use crate::service_manager::ServiceManager;
use open_compute_core::{ErrorCode, PlatformError};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use uuid::Uuid;

pub use crate::release_http::{
    DEFAULT_GITHUB_API_BASE, DEFAULT_RELEASE_DOWNLOAD_BASE, FixtureReleaseHttp, LiveReleaseHttp,
    MAX_BINARY_BYTES, MAX_METADATA_BYTES, RELEASE_HTTP_TIMEOUT, ReleaseHttp,
};

/// Supported formal release targets.
pub const RELEASE_TARGETS: &[&str] = &["darwin-arm64", "linux-arm64", "linux-x64"];

/// Parsed `release.json` identity for one formal tag.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReleaseManifest {
    /// Schema version (must be 1).
    #[serde(rename = "schemaVersion")]
    pub schema_version: u32,
    /// Git tag including the leading `v`.
    pub tag: String,
    /// Stable `SemVer` without `v`.
    pub version: String,
    /// Embedded Git revision.
    #[serde(rename = "gitRevision")]
    pub git_revision: String,
    /// Pinned workerd release label.
    #[serde(rename = "workerdRelease")]
    pub workerd_release: String,
    /// SHA-256 of `workerd.lock.json`.
    #[serde(rename = "workerdLockSha256")]
    pub workerd_lock_sha256: String,
    /// Per-target artifacts.
    pub artifacts: Vec<ReleaseArtifact>,
}

/// One target artifact listed in `release.json`.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReleaseArtifact {
    /// Target token.
    pub target: String,
    /// OS label.
    pub os: String,
    /// Arch label.
    pub arch: String,
    /// Asset filename.
    pub filename: String,
    /// Exact byte length.
    pub bytes: u64,
    /// Lowercase hex SHA-256.
    pub sha256: String,
}

/// Options for [`run_upgrade`].
#[derive(Clone, Debug)]
pub struct UpgradeOptions {
    /// Exact stable `SemVer`, or `None` for latest stable.
    pub version: Option<String>,
    /// Resolve and verify only; do not replace the binary.
    pub dry_run: bool,
    /// Replace the binary but do not restart managed instances.
    pub no_restart: bool,
    /// Absolute path of the binary to replace.
    pub binary_path: PathBuf,
    /// Absolute install receipt path.
    pub receipt_path: PathBuf,
    /// Absolute staging directory on the same filesystem as the binary.
    pub staging_dir: PathBuf,
    /// GitHub download base without a trailing slash.
    pub download_base: String,
    /// GitHub API base without a trailing slash.
    pub api_base: String,
    /// Host release target token.
    pub target: String,
    /// Currently running version string.
    pub current_version: String,
}

impl UpgradeOptions {
    /// Production defaults derived from the running executable.
    pub fn production(
        version: Option<String>,
        dry_run: bool,
        no_restart: bool,
    ) -> Result<Self, PlatformError> {
        let binary_path = std::env::current_exe().map_err(|_| {
            PlatformError::new(
                ErrorCode::PlatformUnavailable,
                "failed to resolve the current ocd executable path",
            )
        })?;
        let binary_path = binary_path.canonicalize().unwrap_or(binary_path);
        let receipt_path = receipt_path_for_binary(&binary_path);
        let staging_dir = binary_path
            .parent()
            .ok_or_else(|| {
                PlatformError::new(
                    ErrorCode::PathInvalid,
                    "binary path has no parent directory",
                )
            })?
            .to_owned();
        Ok(Self {
            version,
            dry_run,
            no_restart,
            binary_path,
            receipt_path,
            staging_dir,
            download_base: DEFAULT_RELEASE_DOWNLOAD_BASE.to_owned(),
            api_base: DEFAULT_GITHUB_API_BASE.to_owned(),
            target: host_release_target()?,
            current_version: env!("CARGO_PKG_VERSION").to_owned(),
        })
    }
}

/// Host target token for the running binary.
pub fn host_release_target() -> Result<String, PlatformError> {
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;
    let target = match (os, arch) {
        ("macos", "aarch64") => "darwin-arm64",
        ("linux", "x86_64") => "linux-x64",
        ("linux", "aarch64") => "linux-arm64",
        _ => {
            return Err(PlatformError::new(
                ErrorCode::ReleaseUnsupported,
                "this host OS/CPU is not a formal open-compute release target",
            ));
        }
    };
    Ok(target.to_owned())
}

/// Resolve latest or exact stable release metadata (no binary download).
pub async fn resolve_release(
    http: &dyn ReleaseHttp,
    api_base: &str,
    download_base: &str,
    version: Option<&str>,
    target: &str,
) -> Result<(ReleaseManifest, ReleaseArtifact, String), PlatformError> {
    if !RELEASE_TARGETS.contains(&target) {
        return Err(PlatformError::new(
            ErrorCode::ReleaseUnsupported,
            "requested release target is not published",
        ));
    }
    let tag = match version {
        Some(value) => {
            if !is_stable_semver(value) {
                return Err(PlatformError::new(
                    ErrorCode::ReleaseUnsupported,
                    "upgrade version must be a stable SemVer X.Y.Z",
                ));
            }
            format!("v{value}")
        }
        None => resolve_latest_stable_tag(http, api_base).await?,
    };
    if !tag.starts_with('v') || !is_stable_semver(tag.trim_start_matches('v')) {
        return Err(PlatformError::new(
            ErrorCode::ReleaseUnsupported,
            "resolved release tag is not a stable SemVer",
        ));
    }
    let base = format!("{download_base}/{tag}");
    let manifest_bytes = http
        .get(&format!("{base}/release.json"), MAX_METADATA_BYTES)
        .await?;
    let sums_bytes = http
        .get(&format!("{base}/SHA256SUMS"), MAX_METADATA_BYTES)
        .await?;
    let manifest = parse_manifest(&manifest_bytes)?;
    if manifest.tag != tag || format!("v{}", manifest.version) != tag {
        return Err(PlatformError::new(
            ErrorCode::ReleaseUnsupported,
            "release.json tag/version does not match the requested release",
        ));
    }
    let sums = parse_sha256sums(&sums_bytes)?;
    verify_checksum(&sums, "release.json", &manifest_bytes)?;
    let artifact = manifest
        .artifacts
        .iter()
        .find(|item| item.target == target)
        .cloned()
        .ok_or_else(|| {
            PlatformError::new(
                ErrorCode::ReleaseUnsupported,
                "release.json does not list an artifact for this target",
            )
        })?;
    let expected = sums.get(&artifact.filename).ok_or_else(|| {
        PlatformError::new(
            ErrorCode::ReleaseUnsupported,
            "SHA256SUMS does not list the target binary",
        )
    })?;
    if expected != &artifact.sha256 {
        return Err(PlatformError::new(
            ErrorCode::ArtifactIntegrityError,
            "release.json sha256 does not match SHA256SUMS",
        ));
    }
    Ok((manifest, artifact, base))
}

/// Execute upgrade (or dry-run) using injectable HTTP and service manager.
pub async fn run_upgrade(
    options: &UpgradeOptions,
    http: &dyn ReleaseHttp,
    registry: &InstanceRegistry,
    manager: &dyn ServiceManager,
    out: &mut impl Write,
) -> Result<(), PlatformError> {
    let receipt = require_upgradeable_receipt(&options.receipt_path, &options.binary_path)?;
    let (manifest, artifact, base) = resolve_release(
        http,
        &options.api_base,
        &options.download_base,
        options.version.as_deref(),
        &options.target,
    )
    .await?;
    match cmp_stable_semver(&manifest.version, &options.current_version) {
        Some(std::cmp::Ordering::Greater) => {}
        Some(std::cmp::Ordering::Equal) => {
            return Err(PlatformError::new(
                ErrorCode::ReleaseUnsupported,
                "requested version is already installed",
            ));
        }
        Some(std::cmp::Ordering::Less) => {
            return Err(PlatformError::new(
                ErrorCode::ReleaseUnsupported,
                "refusing to downgrade; choose a newer stable version",
            ));
        }
        None => {
            return Err(PlatformError::new(
                ErrorCode::ReleaseUnsupported,
                "current or target version is not a stable SemVer",
            ));
        }
    }

    let instances = registry.list()?;
    for record in &instances {
        load_platform_config_from(record.config_path(), Path::new("/"))?;
    }
    let active_instances = if options.no_restart {
        Vec::new()
    } else {
        instances
            .iter()
            .filter_map(|record| match manager.is_active(record) {
                Ok(true) => Some(Ok(record)),
                Ok(false) => None,
                Err(error) => Some(Err(error)),
            })
            .collect::<Result<Vec<_>, PlatformError>>()?
    };
    writeln!(
        out,
        "UPGRADE_PLAN current={} target={} binary={} instances={} restart={} dry_run={}",
        options.current_version,
        manifest.version,
        options.binary_path.display(),
        instances.len(),
        active_instances.len(),
        options.dry_run
    )
    .map_err(|_| io_failed())?;
    for record in &instances {
        writeln!(
            out,
            "UPGRADE_INSTANCE {} {}",
            record.instance_id, record.service_identifier
        )
        .map_err(|_| io_failed())?;
    }
    if options.dry_run {
        writeln!(out, "UPGRADE_DRY_RUN_OK {}", manifest.version).map_err(|_| io_failed())?;
        return Ok(());
    }

    let asset_url = format!("{base}/{}", artifact.filename);
    let bytes = http.get(&asset_url, MAX_BINARY_BYTES).await?;
    if bytes.len() as u64 != artifact.bytes {
        return Err(PlatformError::new(
            ErrorCode::ArtifactIntegrityError,
            "downloaded binary size does not match release.json",
        ));
    }
    let digest = hex::encode(Sha256::digest(&bytes));
    if digest != artifact.sha256 {
        return Err(PlatformError::new(
            ErrorCode::ArtifactIntegrityError,
            "downloaded binary checksum mismatch",
        ));
    }

    fs::create_dir_all(&options.staging_dir).map_err(|_| {
        PlatformError::new(
            ErrorCode::PathInvalid,
            "failed to create upgrade staging directory",
        )
    })?;
    let staged = options
        .staging_dir
        .join(format!(".ocd-upgrade-{}", Uuid::now_v7().as_hyphenated()));
    write_staged_binary(&staged, &bytes)?;
    verify_staged_version(&staged, &manifest.version)?;

    atomic_replace_binary(&staged, &options.binary_path)?;
    let updated = InstallReceipt {
        schema_version: install_receipt::RECEIPT_SCHEMA_VERSION,
        version: manifest.version.clone(),
        sha256: digest,
        target: options.target.clone(),
        binary_path: options.binary_path.to_string_lossy().into_owned(),
        method: receipt.method,
        source: asset_url,
        installed_at_ms: install_receipt::unix_ms_now(SystemTime::now())?,
    };
    write_receipt(&options.receipt_path, &updated)?;

    if options.no_restart {
        writeln!(
            out,
            "UPGRADE_OK {} binary replaced; managed instances were not restarted (--no-restart)",
            manifest.version
        )
        .map_err(|_| io_failed())?;
        return Ok(());
    }

    for record in active_instances {
        if let Err(err) = manager.restart(record) {
            let _ = writeln!(
                out,
                "UPGRADE_INSTANCE_FAILED {} {}",
                record.instance_id,
                err.message()
            );
            return Err(PlatformError::new(
                ErrorCode::PlatformUnavailable,
                "managed instance restart failed after binary replace; remaining instances were not restarted",
            ));
        }
        wait_until_instance_ready_for_release(
            record,
            manager.readiness_runtime_root().as_deref(),
            INSTANCE_READY_TIMEOUT,
            Some(&manifest.version),
        )?;
        writeln!(out, "UPGRADE_INSTANCE_RESTARTED {}", record.instance_id)
            .map_err(|_| io_failed())?;
    }
    writeln!(out, "UPGRADE_OK {}", manifest.version).map_err(|_| io_failed())?;
    Ok(())
}

/// Uninstall the receipt-owned binary and receipt only.
pub fn run_uninstall(
    receipt_path: &Path,
    binary_path: &Path,
    registry: &InstanceRegistry,
    manager: &dyn ServiceManager,
    out: &mut impl Write,
) -> Result<(), PlatformError> {
    let receipt = require_upgradeable_receipt(receipt_path, binary_path)?;
    let records = registry.list()?;
    if !records.is_empty() {
        for record in &records {
            let active = manager.is_active(record).unwrap_or(false);
            writeln!(
                out,
                "UNINSTALL_BLOCKED_INSTANCE {} active={}",
                record.instance_id, active
            )
            .map_err(|_| io_failed())?;
        }
        return Err(PlatformError::new(
            ErrorCode::DataDirInUse,
            "managed instances are still registered; stop them and run `ocd instance remove` before uninstall",
        ));
    }
    if path_looks_package_manager_owned(Path::new(&receipt.binary_path)) {
        return Err(PlatformError::new(
            ErrorCode::ReleaseUnsupported,
            "refusing to uninstall a package-manager path",
        ));
    }
    let owned = PathBuf::from(&receipt.binary_path);
    if owned != binary_path {
        return Err(PlatformError::new(
            ErrorCode::ReleaseUnsupported,
            "install receipt binary_path does not match the running executable",
        ));
    }
    match fs::remove_file(&owned) {
        Ok(()) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => {
            return Err(PlatformError::new(
                ErrorCode::PathInvalid,
                "failed to remove the installed ocd binary",
            ));
        }
    }
    install_receipt::remove_receipt(receipt_path)?;
    writeln!(
        out,
        "UNINSTALL_OK removed {} and {}; config and data were not deleted",
        owned.display(),
        receipt_path.display()
    )
    .map_err(|_| io_failed())?;
    Ok(())
}

/// Build a check result from cache / fresh metadata for Dashboard.
pub async fn check_upgrade_available(
    http: &dyn ReleaseHttp,
    api_base: &str,
    download_base: &str,
    current_version: &str,
    receipt_path: &Path,
    binary_path: &Path,
    target: &str,
) -> Result<crate::upgrade_api::UpgradeCheckResult, PlatformError> {
    let (allowed, blocked) = match require_upgradeable_receipt(receipt_path, binary_path) {
        Ok(_) => (true, None),
        Err(err) => (false, Some(err.message().to_owned())),
    };
    let available = match resolve_release(http, api_base, download_base, None, target).await {
        Ok((manifest, _, _)) => {
            if cmp_stable_semver(&manifest.version, current_version)
                == Some(std::cmp::Ordering::Greater)
            {
                Some(manifest.version)
            } else {
                None
            }
        }
        Err(err) => return Err(err),
    };
    Ok(crate::upgrade_api::check_result(
        current_version,
        available.as_deref(),
        allowed,
        blocked.as_deref(),
    ))
}

async fn resolve_latest_stable_tag(
    http: &dyn ReleaseHttp,
    api_base: &str,
) -> Result<String, PlatformError> {
    let url = format!("{api_base}/repos/elliothux/open-compute/releases/latest");
    let bytes = http.get(&url, MAX_METADATA_BYTES).await?;
    let value: serde_json::Value = serde_json::from_slice(&bytes).map_err(|_| {
        PlatformError::new(
            ErrorCode::ReleaseUnsupported,
            "GitHub latest release JSON is invalid",
        )
    })?;
    if value.get("prerelease").and_then(serde_json::Value::as_bool) == Some(true)
        || value.get("draft").and_then(serde_json::Value::as_bool) == Some(true)
    {
        return Err(PlatformError::new(
            ErrorCode::ReleaseUnsupported,
            "latest GitHub release is a prerelease or draft",
        ));
    }
    let tag = value
        .get("tag_name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            PlatformError::new(
                ErrorCode::ReleaseUnsupported,
                "GitHub latest release is missing tag_name",
            )
        })?;
    Ok(tag.to_owned())
}

fn parse_manifest(bytes: &[u8]) -> Result<ReleaseManifest, PlatformError> {
    let manifest: ReleaseManifest = serde_json::from_slice(bytes).map_err(|_| {
        PlatformError::new(
            ErrorCode::ReleaseUnsupported,
            "release.json is corrupt or invalid",
        )
    })?;
    if manifest.schema_version != 1 {
        return Err(PlatformError::new(
            ErrorCode::ReleaseUnsupported,
            "release.json schema is unsupported",
        ));
    }
    if !is_stable_semver(&manifest.version) {
        return Err(PlatformError::new(
            ErrorCode::ReleaseUnsupported,
            "release.json version is not a stable SemVer",
        ));
    }
    Ok(manifest)
}

fn parse_sha256sums(bytes: &[u8]) -> Result<HashMap<String, String>, PlatformError> {
    let text = std::str::from_utf8(bytes).map_err(|_| {
        PlatformError::new(
            ErrorCode::ReleaseUnsupported,
            "SHA256SUMS is not valid UTF-8",
        )
    })?;
    let mut map = HashMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let mut parts = line.split_whitespace();
        let digest = parts.next().ok_or_else(|| {
            PlatformError::new(
                ErrorCode::ReleaseUnsupported,
                "SHA256SUMS line is malformed",
            )
        })?;
        let name = parts.next().ok_or_else(|| {
            PlatformError::new(
                ErrorCode::ReleaseUnsupported,
                "SHA256SUMS line is malformed",
            )
        })?;
        if parts.next().is_some() || digest.len() != 64 {
            return Err(PlatformError::new(
                ErrorCode::ReleaseUnsupported,
                "SHA256SUMS line is malformed",
            ));
        }
        map.insert(name.to_owned(), digest.to_ascii_lowercase());
    }
    Ok(map)
}

fn verify_checksum(
    sums: &HashMap<String, String>,
    name: &str,
    bytes: &[u8],
) -> Result<(), PlatformError> {
    let expected = sums.get(name).ok_or_else(|| {
        PlatformError::new(
            ErrorCode::ReleaseUnsupported,
            "SHA256SUMS is missing a required file",
        )
    })?;
    let actual = hex::encode(Sha256::digest(bytes));
    if &actual != expected {
        return Err(PlatformError::new(
            ErrorCode::ArtifactIntegrityError,
            "release metadata checksum mismatch",
        ));
    }
    Ok(())
}

fn write_staged_binary(path: &Path, bytes: &[u8]) -> Result<(), PlatformError> {
    if !path.is_absolute() {
        return Err(PlatformError::new(
            ErrorCode::PathInvalid,
            "staging path must be absolute",
        ));
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o755)
        .open(path)
        .map_err(|_| {
            PlatformError::new(
                ErrorCode::PathInvalid,
                "failed to create staged upgrade binary",
            )
        })?;
    file.write_all(bytes).map_err(|_| {
        PlatformError::new(
            ErrorCode::PathInvalid,
            "failed to write staged upgrade binary",
        )
    })?;
    file.sync_all().map_err(|_| {
        PlatformError::new(
            ErrorCode::PathInvalid,
            "failed to fsync staged upgrade binary",
        )
    })?;
    drop(file);
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).map_err(|_| {
        PlatformError::new(
            ErrorCode::PathInvalid,
            "failed to set staged upgrade binary permissions",
        )
    })?;
    Ok(())
}

fn verify_staged_version(path: &Path, expected: &str) -> Result<(), PlatformError> {
    let output = std::process::Command::new(path)
        .arg("--version")
        .output()
        .map_err(|_| {
            PlatformError::new(
                ErrorCode::ReleaseUnsupported,
                "staged binary failed to execute --version",
            )
        })?;
    if !output.status.success() {
        return Err(PlatformError::new(
            ErrorCode::ReleaseUnsupported,
            "staged binary --version exited unsuccessfully",
        ));
    }
    let text = String::from_utf8_lossy(&output.stdout);
    if !text.contains(expected) {
        return Err(PlatformError::new(
            ErrorCode::ReleaseUnsupported,
            "staged binary version does not match the release identity",
        ));
    }
    Ok(())
}

fn atomic_replace_binary(staged: &Path, destination: &Path) -> Result<(), PlatformError> {
    // Same-directory rename keeps the replace on one filesystem.
    if staged.parent() != destination.parent() {
        return Err(PlatformError::new(
            ErrorCode::PathInvalid,
            "upgrade staging directory must be on the same filesystem as the destination",
        ));
    }
    fs::rename(staged, destination).map_err(|_| {
        PlatformError::new(
            ErrorCode::PathInvalid,
            "failed to atomically replace the ocd binary",
        )
    })?;
    if let Some(parent) = destination.parent()
        && let Ok(dir) = File::open(parent)
    {
        let _ = dir.sync_all();
    }
    Ok(())
}

fn io_failed() -> PlatformError {
    PlatformError::new(ErrorCode::Internal, "failed to write upgrade output")
}

/// Read the current install receipt when present (Dashboard / CLI helpers).
pub fn load_receipt_for_exe() -> Result<(PathBuf, PathBuf, InstallReceipt), PlatformError> {
    let binary = std::env::current_exe().map_err(|_| {
        PlatformError::new(
            ErrorCode::PlatformUnavailable,
            "failed to resolve the current ocd executable path",
        )
    })?;
    let binary = binary.canonicalize().unwrap_or(binary);
    let receipt_path = receipt_path_for_binary(&binary);
    let receipt = read_receipt(&receipt_path)?;
    Ok((receipt_path, binary, receipt))
}

#[cfg(test)]
#[path = "release_upgrade_tests.rs"]
mod tests;
