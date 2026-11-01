//! Install receipt for formal `ocd` binary ownership.

use open_compute_core::{ErrorCode, PlatformError};
use open_compute_storage::fs::{atomic_write, ensure_dir_secure};
use serde::{Deserialize, Serialize};
use std::fs;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Current install-receipt schema.
pub const RECEIPT_SCHEMA_VERSION: u32 = 1;

/// Receipt filename inside the selected OCD scope root.
pub const INSTALL_RECEIPT_NAME: &str = "install-receipt.json";

/// Methods that may be upgraded/uninstalled by `ocd` itself.
pub const SELF_MANAGED_METHODS: &[&str] = &["install.sh", "manual"];

/// Secret-free record written by `scripts/install.sh` or an operator.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct InstallReceipt {
    /// Schema version.
    pub schema_version: u32,
    /// Installed stable `SemVer` without a leading `v`.
    pub version: String,
    /// Release target triple token (`darwin-arm64`, `linux-x64`, …).
    pub target: String,
    /// Absolute path of the installed `ocd` binary.
    pub binary_path: String,
    /// Install channel (`install.sh`, `manual`, or a package-manager token).
    pub method: String,
    /// Immutable release asset URL or local source label (no secrets).
    pub source: String,
    /// Unix epoch milliseconds when the receipt was written.
    pub installed_at_ms: u64,
}

impl InstallReceipt {
    /// Validate schema and absolute binary path form.
    pub fn validate(&self) -> Result<(), PlatformError> {
        if self.schema_version != RECEIPT_SCHEMA_VERSION {
            return Err(PlatformError::new(
                ErrorCode::ReleaseUnsupported,
                "install receipt schema is unsupported",
            ));
        }
        if !is_stable_semver(&self.version) {
            return Err(PlatformError::new(
                ErrorCode::ReleaseUnsupported,
                "install receipt version is not a stable SemVer",
            ));
        }
        if self.target.is_empty() || self.method.is_empty() || self.source.is_empty() {
            return Err(PlatformError::new(
                ErrorCode::ReleaseUnsupported,
                "install receipt is missing required fields",
            ));
        }
        let path = Path::new(&self.binary_path);
        if !path.is_absolute() {
            return Err(PlatformError::new(
                ErrorCode::PathInvalid,
                "install receipt binary_path must be absolute",
            ));
        }
        Ok(())
    }

    /// Whether this receipt may be upgraded or uninstalled by `ocd`.
    #[must_use]
    pub fn is_self_managed(&self) -> bool {
        SELF_MANAGED_METHODS
            .iter()
            .any(|method| method == &self.method)
    }
}

/// Resolve the receipt within the selected OCD scope root.
#[must_use]
pub fn receipt_path_in(ocd_root: &Path) -> PathBuf {
    ocd_root.join(INSTALL_RECEIPT_NAME)
}

/// Read and validate a receipt; ignore corrupt files by returning an error.
pub fn read_receipt(path: &Path) -> Result<InstallReceipt, PlatformError> {
    require_absolute(path)?;
    let mut file = File::open(path).map_err(|_| {
        PlatformError::new(ErrorCode::PathInvalid, "failed to open install receipt")
    })?;
    let opened = file.metadata().map_err(|_| {
        PlatformError::new(ErrorCode::PathInvalid, "failed to inspect install receipt")
    })?;
    if !opened.is_file() {
        return Err(PlatformError::new(
            ErrorCode::PathInvalid,
            "install receipt path is not a regular file",
        ));
    }
    if opened.len() > 16 * 1024 {
        return Err(PlatformError::new(
            ErrorCode::PathInvalid,
            "install receipt exceeds the size bound",
        ));
    }
    let mut bytes = Vec::new();
    file.by_ref()
        .take(16 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| {
            PlatformError::new(ErrorCode::PathInvalid, "failed to read install receipt")
        })?;
    if bytes.len() > 16 * 1024 {
        return Err(PlatformError::new(
            ErrorCode::PathInvalid,
            "install receipt exceeds the size bound",
        ));
    }
    let receipt: InstallReceipt = serde_json::from_slice(&bytes).map_err(|_| {
        PlatformError::new(
            ErrorCode::ReleaseUnsupported,
            "install receipt JSON is corrupt or invalid",
        )
    })?;
    receipt.validate()?;
    Ok(receipt)
}

/// Atomically write a new receipt. Refuses to overwrite a different method's receipt.
pub fn write_receipt(path: &Path, receipt: &InstallReceipt) -> Result<(), PlatformError> {
    receipt.validate()?;
    require_absolute(path)?;
    let parent = path.parent().ok_or_else(|| {
        PlatformError::new(
            ErrorCode::PathInvalid,
            "install receipt path must have a parent directory",
        )
    })?;
    ensure_receipt_parent(parent)?;
    if path.exists() {
        match read_receipt(path) {
            Ok(existing) => {
                if existing.method != receipt.method
                    && (!existing.is_self_managed() || !receipt.is_self_managed())
                {
                    return Err(PlatformError::new(
                        ErrorCode::PathInvalid,
                        "refusing to overwrite a package-manager-owned install receipt",
                    ));
                }
                if !existing.is_self_managed() {
                    return Err(PlatformError::new(
                        ErrorCode::PathInvalid,
                        "refusing to overwrite a package-manager-owned install receipt",
                    ));
                }
            }
            Err(_) => {
                return Err(PlatformError::new(
                    ErrorCode::PathInvalid,
                    "refusing to overwrite an unreadable existing install receipt",
                ));
            }
        }
    }
    let bytes = serde_json::to_vec_pretty(receipt).map_err(|_| {
        PlatformError::new(ErrorCode::Internal, "failed to serialize install receipt")
    })?;
    atomic_write(path, &bytes)
}

/// Delete a receipt file when present.
pub fn remove_receipt(path: &Path) -> Result<(), PlatformError> {
    require_absolute(path)?;
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(PlatformError::new(
            ErrorCode::PathInvalid,
            "failed to remove install receipt",
        )),
    }
}

/// True when the path looks owned by Homebrew, APT system bins, or similar.
#[must_use]
pub fn path_looks_package_manager_owned(path: &Path) -> bool {
    let text = path.to_string_lossy();
    text.contains("/Cellar/")
        || text.contains("/opt/homebrew/")
        || text.contains("/linuxbrew/")
        || text.contains("/.linuxbrew/")
        || text == "/usr/bin/ocd"
}

/// Reject upgrades for package-manager installs or missing/foreign receipts.
pub fn require_upgradeable_receipt(
    receipt_path: &Path,
    binary_path: &Path,
) -> Result<InstallReceipt, PlatformError> {
    if path_looks_package_manager_owned(binary_path) {
        return Err(PlatformError::new(
            ErrorCode::ReleaseUnsupported,
            "this ocd binary appears package-manager-owned; use the package manager to upgrade",
        ));
    }
    let receipt = read_receipt(receipt_path).map_err(|_| {
        PlatformError::new(
            ErrorCode::ReleaseUnsupported,
            "install receipt is missing; only install.sh/manual installs may be upgraded by ocd",
        )
    })?;
    if !receipt.is_self_managed() {
        return Err(PlatformError::new(
            ErrorCode::ReleaseUnsupported,
            "install receipt method is package-manager-owned; use the package manager to upgrade",
        ));
    }
    if Path::new(&receipt.binary_path) != binary_path {
        return Err(PlatformError::new(
            ErrorCode::ReleaseUnsupported,
            "install receipt binary_path does not match the running executable",
        ));
    }
    Ok(receipt)
}

/// Unix milliseconds for receipt timestamps.
pub fn unix_ms_now(now: SystemTime) -> Result<u64, PlatformError> {
    let ms = open_compute_core::unix_time_ms(now).ok_or_else(|| {
        PlatformError::new(
            ErrorCode::Internal,
            "system clock is outside the supported Unix timestamp range",
        )
    })?;
    u64::try_from(ms).map_err(|_| PlatformError::new(ErrorCode::Internal, "timestamp overflow"))
}

/// Stable `SemVer` `X.Y.Z` without prerelease or build metadata.
#[must_use]
pub fn is_stable_semver(value: &str) -> bool {
    parse_stable_semver(value).is_some()
}

/// Compare two stable `SemVer` strings. Returns `None` when either is invalid.
#[must_use]
pub fn cmp_stable_semver(left: &str, right: &str) -> Option<std::cmp::Ordering> {
    Some(parse_stable_semver(left)?.cmp(&parse_stable_semver(right)?))
}

/// Parse `X.Y.Z` into sortable components.
#[must_use]
pub fn parse_stable_semver(value: &str) -> Option<(u64, u64, u64)> {
    let version = semver::Version::parse(value).ok()?;
    (version.pre.is_empty() && version.build.is_empty()).then_some((
        version.major,
        version.minor,
        version.patch,
    ))
}

fn require_absolute(path: &Path) -> Result<(), PlatformError> {
    if path.is_absolute() {
        Ok(())
    } else {
        Err(PlatformError::new(
            ErrorCode::PathInvalid,
            "path must be absolute",
        ))
    }
}

fn ensure_receipt_parent(parent: &Path) -> Result<(), PlatformError> {
    require_absolute(parent)?;
    ensure_dir_secure(parent)?;
    Ok(())
}

#[cfg(test)]
#[path = "install_receipt_tests.rs"]
mod tests;
