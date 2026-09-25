//! Formal release download, `ocd upgrade`, and `ocd uninstall`.

use crate::install_receipt::{
    self, InstallReceipt, cmp_stable_semver, is_stable_semver, path_looks_package_manager_owned,
    receipt_path_in, require_upgradeable_receipt, write_receipt,
};
use crate::instance_ops::wait_scoped_daemon_state;
use crate::instance_registry::{InstanceRegistry, ServiceScope};
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

mod backups;
use backups::UpgradeBackups;
#[cfg(test)]
use backups::backup_path;
pub use backups::run_upgrade_restore;

pub use crate::release_http::{
    DEFAULT_RELEASE_DOWNLOAD_BASE, FixtureReleaseHttp, LiveReleaseHttp, MAX_BINARY_BYTES,
    MAX_METADATA_BYTES, RELEASE_HTTP_TIMEOUT, ReleaseHttp,
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
    /// Explicit OCD scope whose one service is upgraded.
    pub scope: ServiceScope,
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
        scope: ServiceScope,
    ) -> Result<Self, PlatformError> {
        let binary_path = std::env::current_exe().map_err(|_| {
            PlatformError::new(
                ErrorCode::PlatformUnavailable,
                "failed to resolve the current ocd executable path",
            )
        })?;
        let binary_path = binary_path.canonicalize().unwrap_or(binary_path);
        let receipt_path = receipt_path_in(InstanceRegistry::production()?.root_for(scope));
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
            scope,
            version,
            dry_run,
            no_restart,
            binary_path,
            receipt_path,
            staging_dir,
            download_base: DEFAULT_RELEASE_DOWNLOAD_BASE.to_owned(),
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
    let (tag, manifest_bytes) = match version {
        Some(value) => {
            if !is_stable_semver(value) {
                return Err(PlatformError::new(
                    ErrorCode::ReleaseUnsupported,
                    "upgrade version must be a stable SemVer X.Y.Z",
                ));
            }
            let tag = format!("v{value}");
            let bytes = http
                .get(
                    &format!("{download_base}/{tag}/release.json"),
                    MAX_METADATA_BYTES,
                )
                .await?;
            (tag, bytes)
        }
        None => {
            let releases_base = download_base.strip_suffix("/download").ok_or_else(|| {
                PlatformError::new(
                    ErrorCode::ReleaseUnsupported,
                    "release download base is invalid",
                )
            })?;
            let bytes = http
                .get(
                    &format!("{releases_base}/latest/download/release.json"),
                    MAX_METADATA_BYTES,
                )
                .await?;
            let manifest = parse_manifest(&bytes)?;
            if manifest.tag != format!("v{}", manifest.version) {
                return Err(PlatformError::new(
                    ErrorCode::ReleaseUnsupported,
                    "latest release.json tag/version is inconsistent",
                ));
            }
            (manifest.tag.clone(), bytes)
        }
    };
    if !tag.starts_with('v') || !is_stable_semver(tag.trim_start_matches('v')) {
        return Err(PlatformError::new(
            ErrorCode::ReleaseUnsupported,
            "resolved release tag is not a stable SemVer",
        ));
    }
    let base = format!("{download_base}/{tag}");
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
    UpgradeBackups::new(options)?.ensure_absent()?;
    let (manifest, artifact, base) = resolve_release(
        http,
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

    let instances = registry.list_scope(options.scope)?;
    let daemon_active = manager.is_active(options.scope)?;
    let restart = !options.no_restart && daemon_active;
    for record in &instances {
        if let Err(error) = registry.validate_registered_config(record) {
            writeln!(
                out,
                "UPGRADE_INVALID_INSTANCE {} config={} error={} recovery='restore the config or stop the daemon and edit ocd.toml'",
                record.instance_id,
                record.config_path().display(),
                error.code().as_str(),
            )
            .map_err(|_| io_failed())?;
            return Err(PlatformError::new(
                ErrorCode::InstanceRegistryInvalid,
                "an owned instance has invalid configuration; resolve the reported config before retrying",
            ));
        }
    }
    writeln!(
        out,
        "UPGRADE_PLAN current={} target={} binary={} instances={} restart={} dry_run={}",
        options.current_version,
        manifest.version,
        options.binary_path.display(),
        instances.len(),
        restart,
        options.dry_run
    )
    .map_err(|_| io_failed())?;
    for record in &instances {
        writeln!(
            out,
            "UPGRADE_INSTANCE {} {}",
            record.instance_id,
            record.config_path().display()
        )
        .map_err(|_| io_failed())?;
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
    if daemon_active {
        for record in &instances {
            verify_staged_instance(&staged, record.config_path())?;
        }
    }
    if options.dry_run {
        fs::remove_file(&staged).map_err(|_| {
            PlatformError::new(
                ErrorCode::PathInvalid,
                "failed to remove staged upgrade binary",
            )
        })?;
        writeln!(out, "UPGRADE_DRY_RUN_OK {}", manifest.version).map_err(|_| io_failed())?;
        return Ok(());
    }

    let backups = UpgradeBackups::create(options)?;
    if let Err(error) = atomic_replace_binary(&staged, &options.binary_path) {
        let _ = backups.remove();
        return Err(error);
    }
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
    if let Err(error) = write_receipt(&options.receipt_path, &updated) {
        if backups.restore(options).is_ok() {
            let _ = backups.remove();
        }
        return Err(error);
    }

    if options.no_restart {
        writeln!(
            out,
            "UPGRADE_OK {} binary replaced; scoped daemon was not restarted (--no-restart)",
            manifest.version
        )
        .map_err(|_| io_failed())?;
        backups.remove()?;
        return Ok(());
    }

    if restart {
        if let Err(error) = manager
            .restart(options.scope)
            .and_then(|()| wait_scoped_daemon_state(registry, manager, options.scope, true))
        {
            let recovery = backups.restore(options).and_then(|()| {
                manager
                    .restart(options.scope)
                    .and_then(|()| wait_scoped_daemon_state(registry, manager, options.scope, true))
            });
            if let Err(recovery) = recovery {
                writeln!(
                    out,
                    "UPGRADE_ROLLBACK_FAILED primary={} recovery={} backup_binary={} backup_receipt={}",
                    error.code().as_str(),
                    recovery.code().as_str(),
                    backups.binary.display(),
                    backups.receipt.display(),
                )
                .map_err(|_| io_failed())?;
            } else {
                backups.remove()?;
            }
            return Err(error);
        }
        writeln!(out, "UPGRADE_DAEMON_RESTARTED {}", options.scope.as_str())
            .map_err(|_| io_failed())?;
    }
    backups.remove()?;
    writeln!(out, "UPGRADE_OK {}", manifest.version).map_err(|_| io_failed())?;
    Ok(())
}

/// Data-removal and confirmation controls for [`run_uninstall`].
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct UninstallOptions {
    /// Irreversibly remove proven local instance state.
    pub purge: bool,
    /// Confirm the destructive purge without an interactive prompt.
    pub yes: bool,
    /// Print the complete plan without mutating the host.
    pub dry_run: bool,
}

/// Uninstall the receipt-owned binary and receipt only.
pub fn run_uninstall(
    receipt_path: &Path,
    binary_path: &Path,
    registry: &InstanceRegistry,
    manager: &dyn ServiceManager,
    scope: ServiceScope,
    options: UninstallOptions,
    out: &mut impl Write,
) -> Result<(), PlatformError> {
    let UninstallOptions {
        purge,
        yes,
        dry_run,
    } = options;
    let receipt = require_upgradeable_receipt(receipt_path, binary_path)?;
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
    let records = registry.list_scope(scope)?;
    writeln!(
        out,
        "UNINSTALL_PLAN binary={} receipt={} instances={} purge={} dry_run={}",
        owned.display(),
        receipt_path.display(),
        records.len(),
        purge,
        dry_run
    )
    .map_err(|_| io_failed())?;
    if purge {
        crate::instance_purge::purge_records(
            &records,
            registry,
            manager,
            yes,
            true,
            &mut std::io::sink(),
        )?;
    } else {
        crate::instance_purge::unregister_preserving_data(
            &records,
            registry,
            manager,
            true,
            &mut std::io::sink(),
        )?;
    }
    if !dry_run {
        if manager.is_active(scope)? {
            manager.stop(scope)?;
            wait_scoped_daemon_state(registry, manager, scope, false)?;
        }
        manager.uninstall(scope)?;
    }
    if purge {
        crate::instance_purge::purge_records(&records, registry, manager, yes, dry_run, out)?;
    } else {
        crate::instance_purge::unregister_preserving_data(
            &records, registry, manager, dry_run, out,
        )?;
    }
    if dry_run {
        writeln!(out, "UNINSTALL_DRY_RUN_OK").map_err(|_| io_failed())?;
        return Ok(());
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
    if purge {
        writeln!(
            out,
            "UNINSTALL_OK removed {} and {}; planned local state was purged and reported external authorities were retained",
            owned.display(),
            receipt_path.display()
        )
        .map_err(|_| io_failed())?;
    } else {
        writeln!(
            out,
            "UNINSTALL_OK removed {} and {}; config and data were not deleted",
            owned.display(),
            receipt_path.display()
        )
        .map_err(|_| io_failed())?;
    }
    Ok(())
}

/// Build a check result from cache / fresh metadata for Dashboard.
pub async fn check_upgrade_available(
    http: &dyn ReleaseHttp,
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
    let available = match resolve_release(http, download_base, None, target).await {
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

fn verify_staged_instance(path: &Path, config: &Path) -> Result<(), PlatformError> {
    let status = std::process::Command::new(path)
        .args(["--no-update-check", "--config"])
        .arg(config)
        .arg("__upgrade_preflight")
        .status()
        .map_err(|_| {
            PlatformError::new(
                ErrorCode::MigrationFailed,
                "staged binary upgrade preflight failed to execute",
            )
        })?;
    if !status.success() {
        return Err(PlatformError::new(
            ErrorCode::MigrationFailed,
            "staged binary rejected an active instance during upgrade preflight",
        ));
    }
    Ok(())
}

fn sync_parent(path: &Path) {
    if let Some(parent) = path.parent()
        && let Ok(dir) = File::open(parent)
    {
        let _ = dir.sync_all();
    }
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
    sync_parent(destination);
    Ok(())
}

fn io_failed() -> PlatformError {
    PlatformError::new(ErrorCode::Internal, "failed to write upgrade output")
}

#[cfg(test)]
mod tests;
