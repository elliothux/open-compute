//! Install receipt for formal `ocd` binary ownership.

use open_compute_core::{ErrorCode, PlatformError};
use open_compute_storage::atomic_write;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::fs::File;
use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Current install-receipt schema.
pub const RECEIPT_SCHEMA_VERSION: u32 = 1;

/// Default system receipt beside a `/usr/local` prefix install.
pub const DEFAULT_RECEIPT_PATH: &str = "/usr/local/share/open-compute/install-receipt.json";

/// Methods that may be upgraded/uninstalled by `ocd` itself.
pub const SELF_MANAGED_METHODS: &[&str] = &["install.sh", "manual"];

/// Secret-free record written by `scripts/install.sh` or an operator.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct InstallReceipt {
    /// Schema version.
    pub schema_version: u32,
    /// Installed stable `SemVer` without a leading `v`.
    pub version: String,
    /// Lowercase hex SHA-256 of the installed binary bytes.
    pub sha256: String,
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
        if !is_sha256_hex(&self.sha256) {
            return Err(PlatformError::new(
                ErrorCode::ReleaseUnsupported,
                "install receipt sha256 is invalid",
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

/// Resolve the default receipt path for a binary under `$prefix/bin/ocd`.
#[must_use]
pub fn receipt_path_for_binary(binary: &Path) -> PathBuf {
    binary.parent().and_then(Path::parent).map_or_else(
        || PathBuf::from(DEFAULT_RECEIPT_PATH),
        |prefix| prefix.join("share/open-compute/install-receipt.json"),
    )
}

/// Production receipt path for the running executable.
pub fn production_receipt_path() -> Result<PathBuf, PlatformError> {
    let exe = std::env::current_exe().map_err(|_| {
        PlatformError::new(
            ErrorCode::PlatformUnavailable,
            "failed to resolve the current ocd executable path",
        )
    })?;
    Ok(receipt_path_for_binary(&exe))
}

/// Read and validate a receipt; ignore corrupt files by returning an error.
pub fn read_receipt(path: &Path) -> Result<InstallReceipt, PlatformError> {
    require_absolute(path)?;
    let meta = fs::symlink_metadata(path)
        .map_err(|_| PlatformError::new(ErrorCode::PathInvalid, "install receipt is missing"))?;
    if meta.file_type().is_symlink() {
        return Err(PlatformError::new(
            ErrorCode::PathInvalid,
            "install receipt must not be a symlink",
        ));
    }
    if !meta.is_file() {
        return Err(PlatformError::new(
            ErrorCode::PathInvalid,
            "install receipt path is not a regular file",
        ));
    }
    let bytes = fs::read(path).map_err(|_| {
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
    verify_receipt_binary(binary_path, &receipt.sha256)?;
    Ok(receipt)
}

fn verify_receipt_binary(path: &Path, expected_sha256: &str) -> Result<(), PlatformError> {
    let metadata = fs::symlink_metadata(path).map_err(|_| {
        PlatformError::new(
            ErrorCode::ReleaseUnsupported,
            "installed binary named by the receipt is missing",
        )
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(PlatformError::new(
            ErrorCode::ReleaseUnsupported,
            "installed binary named by the receipt must be a regular file",
        ));
    }
    let mut file = File::open(path).map_err(|_| {
        PlatformError::new(
            ErrorCode::ReleaseUnsupported,
            "installed binary named by the receipt is unreadable",
        )
    })?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(|_| {
            PlatformError::new(
                ErrorCode::ReleaseUnsupported,
                "failed to verify the installed binary against its receipt",
            )
        })?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    if hex::encode(hasher.finalize()) != expected_sha256 {
        return Err(PlatformError::new(
            ErrorCode::ReleaseUnsupported,
            "installed binary sha256 does not match its install receipt",
        ));
    }
    Ok(())
}

/// Unix milliseconds for receipt timestamps.
pub fn unix_ms_now(now: SystemTime) -> Result<u64, PlatformError> {
    let ms = now
        .duration_since(UNIX_EPOCH)
        .map_err(|_| {
            PlatformError::new(ErrorCode::Internal, "system clock is before the Unix epoch")
        })?
        .as_millis();
    u64::try_from(ms).map_err(|_| PlatformError::new(ErrorCode::Internal, "timestamp overflow"))
}

/// Stable `SemVer` `X.Y.Z` without prerelease or build metadata.
#[must_use]
pub fn is_stable_semver(value: &str) -> bool {
    let mut parts = value.split('.');
    let Some(major) = parts.next() else {
        return false;
    };
    let Some(minor) = parts.next() else {
        return false;
    };
    let Some(patch) = parts.next() else {
        return false;
    };
    if parts.next().is_some() {
        return false;
    }
    parse_numeric_component(major).is_some()
        && parse_numeric_component(minor).is_some()
        && parse_numeric_component(patch).is_some()
}

/// Compare two stable `SemVer` strings. Returns `None` when either is invalid.
#[must_use]
pub fn cmp_stable_semver(left: &str, right: &str) -> Option<std::cmp::Ordering> {
    let l = parse_stable_semver(left)?;
    let r = parse_stable_semver(right)?;
    Some(l.cmp(&r))
}

/// Parse `X.Y.Z` into sortable components.
#[must_use]
pub fn parse_stable_semver(value: &str) -> Option<(u64, u64, u64)> {
    let mut parts = value.split('.');
    let major = parse_numeric_component(parts.next()?)?;
    let minor = parse_numeric_component(parts.next()?)?;
    let patch = parse_numeric_component(parts.next()?)?;
    if parts.next().is_some() {
        return None;
    }
    Some((major, minor, patch))
}

fn parse_numeric_component(value: &str) -> Option<u64> {
    if value.is_empty() || (value.len() > 1 && value.starts_with('0')) {
        return None;
    }
    if !value.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    value.parse().ok()
}

fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit())
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
    if parent.exists() {
        let meta = fs::symlink_metadata(parent).map_err(|_| {
            PlatformError::new(
                ErrorCode::PathInvalid,
                "install receipt parent is not accessible",
            )
        })?;
        if meta.file_type().is_symlink() || !meta.is_dir() {
            return Err(PlatformError::new(
                ErrorCode::PathInvalid,
                "install receipt parent must be a real directory",
            ));
        }
        return Ok(());
    }
    fs::create_dir_all(parent).map_err(|_| {
        PlatformError::new(
            ErrorCode::PathInvalid,
            "failed to create install receipt parent directory",
        )
    })?;
    let _ = fs::set_permissions(parent, fs::Permissions::from_mode(0o755));
    Ok(())
}

#[cfg(test)]
#[path = "install_receipt_tests.rs"]
mod tests;
