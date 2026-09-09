//! Asynchronous update-check cache and detached helper for CLI reminders.

use crate::install_receipt::{cmp_stable_semver, is_stable_semver};
use crate::release_upgrade::{
    DEFAULT_GITHUB_API_BASE, DEFAULT_RELEASE_DOWNLOAD_BASE, ReleaseHttp, check_upgrade_available,
};
use open_compute_core::{ErrorCode, PlatformError};
use open_compute_storage::atomic_write;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime};

/// Successful checks cool down for 24 hours.
pub const SUCCESS_COOLDOWN: Duration = Duration::from_secs(24 * 60 * 60);
/// Failed checks cool down for at least 1 hour.
pub const FAILURE_COOLDOWN: Duration = Duration::from_secs(60 * 60);

const CACHE_SCHEMA: u32 = 1;
const MAX_CACHE_BYTES: usize = 16 * 1024;

/// Bounded on-disk update-check cache (non-authoritative).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct UpdateCheckCache {
    /// Schema version.
    pub schema_version: u32,
    /// Currently observed local version when the check ran.
    pub current_version: String,
    /// Newest stable version discovered, if any.
    pub latest_version: Option<String>,
    /// Unix ms of the last successful metadata check.
    pub checked_at_ms: Option<u64>,
    /// Unix ms of the last failed metadata check.
    pub failed_at_ms: Option<u64>,
}

impl UpdateCheckCache {
    /// Empty cache for the running version.
    #[must_use]
    pub fn empty(current_version: impl Into<String>) -> Self {
        Self {
            schema_version: CACHE_SCHEMA,
            current_version: current_version.into(),
            latest_version: None,
            checked_at_ms: None,
            failed_at_ms: None,
        }
    }

    /// Whether the cache records a successful metadata check.
    #[must_use]
    pub fn success(&self) -> bool {
        self.checked_at_ms.is_some()
    }
}

/// Resolve the per-user cache file path.
pub fn default_cache_path() -> Result<PathBuf, PlatformError> {
    let base = user_cache_root()?;
    Ok(base.join("update-check.json"))
}

fn user_cache_root() -> Result<PathBuf, PlatformError> {
    if let Ok(xdg) = std::env::var("XDG_CACHE_HOME")
        && !xdg.is_empty()
    {
        return Ok(PathBuf::from(xdg).join("open-compute"));
    }
    #[cfg(target_os = "macos")]
    {
        if let Ok(home) = std::env::var("HOME") {
            return Ok(PathBuf::from(home).join("Library/Caches/dev.open-compute"));
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        if let Ok(home) = std::env::var("HOME") {
            return Ok(PathBuf::from(home).join(".cache/open-compute"));
        }
    }
    Err(PlatformError::new(
        ErrorCode::PathInvalid,
        "failed to resolve the update-check cache directory",
    ))
}

/// Read cache; corrupt / symlink / oversized files are ignored.
pub fn read_cache(path: &Path) -> Option<UpdateCheckCache> {
    let meta = fs::symlink_metadata(path).ok()?;
    if meta.file_type().is_symlink() || !meta.is_file() {
        return None;
    }
    if meta.permissions().mode() & 0o077 != 0 || meta.uid() != rustix::process::getuid().as_raw() {
        return None;
    }
    let bytes = fs::read(path).ok()?;
    if bytes.len() > MAX_CACHE_BYTES {
        return None;
    }
    let cache: UpdateCheckCache = serde_json::from_slice(&bytes).ok()?;
    if cache.schema_version != CACHE_SCHEMA {
        return None;
    }
    if !is_stable_semver(&cache.current_version) {
        return None;
    }
    if let Some(latest) = &cache.latest_version
        && !is_stable_semver(latest)
    {
        return None;
    }
    let now_ms = unix_ms(SystemTime::now()).ok()?;
    if cache
        .checked_at_ms
        .is_some_and(|timestamp| timestamp > now_ms)
        || cache
            .failed_at_ms
            .is_some_and(|timestamp| timestamp > now_ms)
    {
        return None;
    }
    Some(cache)
}

/// Atomically write a validated cache document.
pub fn write_cache(path: &Path, cache: &UpdateCheckCache) -> Result<(), PlatformError> {
    if cache.schema_version != CACHE_SCHEMA {
        return Err(PlatformError::new(
            ErrorCode::ReleaseUnsupported,
            "update-check cache schema is unsupported",
        ));
    }
    let parent = path.parent().ok_or_else(|| {
        PlatformError::new(
            ErrorCode::PathInvalid,
            "update-check cache path must have a parent",
        )
    })?;
    fs::create_dir_all(parent).map_err(|_| {
        PlatformError::new(
            ErrorCode::PathInvalid,
            "failed to create update-check cache directory",
        )
    })?;
    let bytes = serde_json::to_vec_pretty(cache).map_err(|_| {
        PlatformError::new(
            ErrorCode::Internal,
            "failed to serialize update-check cache",
        )
    })?;
    atomic_write(path, &bytes)
}

/// Whether a detached helper should refresh metadata now.
#[must_use]
pub fn should_refresh(cache: Option<&UpdateCheckCache>, now: SystemTime) -> bool {
    let Ok(now_ms) = unix_ms(now) else {
        return false;
    };
    let Some(cache) = cache else {
        return true;
    };
    if let Some(checked) = cache.checked_at_ms
        && now_ms.saturating_sub(checked) < SUCCESS_COOLDOWN.as_millis() as u64
    {
        return false;
    }
    if let Some(failed) = cache.failed_at_ms
        && now_ms.saturating_sub(failed) < FAILURE_COOLDOWN.as_millis() as u64
        && cache.checked_at_ms.is_none_or(|checked| checked < failed)
    {
        return false;
    }
    true
}

/// Print a one-line TTY reminder when the cache proves a newer stable version.
pub fn maybe_print_reminder(
    cache: &UpdateCheckCache,
    current_version: &str,
    stderr_is_tty: bool,
    stderr: &mut impl Write,
) {
    if !stderr_is_tty {
        return;
    }
    let Some(latest) = cache.latest_version.as_deref() else {
        return;
    };
    if cmp_stable_semver(latest, current_version) != Some(std::cmp::Ordering::Greater) {
        return;
    }
    let _ = writeln!(
        stderr,
        "Update available: {latest} (current {current_version}). Run: sudo ocd upgrade"
    );
}

/// Spawn a detached `__update_check` helper using the absolute current executable.
pub fn spawn_detached_helper(exe: &Path) -> Result<(), PlatformError> {
    Command::new(exe)
        .arg("__update_check")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| {
            PlatformError::new(
                ErrorCode::Internal,
                "failed to spawn the detached update-check helper",
            )
        })?;
    Ok(())
}

/// Pre-command hook for interactive management CLI invocations.
pub fn pre_command_update_check(
    no_update_check: bool,
    allow_network_refresh: bool,
    current_version: &str,
    cache_path: &Path,
    exe: &Path,
    stderr_is_tty: bool,
    stderr: &mut impl Write,
) {
    if no_update_check {
        return;
    }
    let cache = read_cache(cache_path);
    if let Some(cache) = &cache {
        maybe_print_reminder(cache, current_version, stderr_is_tty, stderr);
    }
    if !allow_network_refresh {
        return;
    }
    if should_refresh(cache.as_ref(), SystemTime::now()) {
        let _ = spawn_detached_helper(exe);
    }
}

/// Production helper entry used by `ocd __update_check`.
pub async fn run_update_check_helper_live(cache_path: &Path) -> Result<(), PlatformError> {
    let http = crate::release_upgrade::LiveReleaseHttp::new()?;
    let binary_path = std::env::current_exe().map_err(|_| {
        PlatformError::new(
            ErrorCode::PlatformUnavailable,
            "failed to resolve the current ocd executable path",
        )
    })?;
    let receipt_path = crate::install_receipt::receipt_path_for_binary(&binary_path);
    let target = crate::release_upgrade::host_release_target()?;
    run_update_check_helper(
        &http,
        cache_path,
        env!("CARGO_PKG_VERSION"),
        &receipt_path,
        &binary_path,
        &target,
    )
    .await
}

/// Hidden helper entry: refresh cache from formal release metadata only.
pub async fn run_update_check_helper(
    http: &dyn ReleaseHttp,
    cache_path: &Path,
    current_version: &str,
    receipt_path: &Path,
    binary_path: &Path,
    target: &str,
) -> Result<(), PlatformError> {
    let now = SystemTime::now();
    match check_upgrade_available(
        http,
        DEFAULT_GITHUB_API_BASE,
        DEFAULT_RELEASE_DOWNLOAD_BASE,
        current_version,
        receipt_path,
        binary_path,
        target,
    )
    .await
    {
        Ok(result) => {
            let cache = UpdateCheckCache {
                schema_version: CACHE_SCHEMA,
                current_version: current_version.to_owned(),
                latest_version: result
                    .available_version
                    .or_else(|| Some(current_version.to_owned())),
                checked_at_ms: Some(unix_ms(now)?),
                failed_at_ms: None,
            };
            write_cache(cache_path, &cache)
        }
        Err(_) => {
            let mut cache =
                read_cache(cache_path).unwrap_or_else(|| UpdateCheckCache::empty(current_version));
            cache.failed_at_ms = Some(unix_ms(now)?);
            write_cache(cache_path, &cache)
        }
    }
}

fn unix_ms(now: SystemTime) -> Result<u64, PlatformError> {
    let ms = open_compute_core::unix_time_ms(now).ok_or_else(|| {
        PlatformError::new(
            ErrorCode::Internal,
            "system clock is outside the supported Unix timestamp range",
        )
    })?;
    u64::try_from(ms).map_err(|_| PlatformError::new(ErrorCode::Internal, "timestamp overflow"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::UNIX_EPOCH;
    use tempfile::TempDir;

    #[test]
    fn corrupt_cache_is_ignored() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("update-check.json");
        fs::write(&path, b"{nope").unwrap();
        assert!(read_cache(&path).is_none());
    }

    #[test]
    fn cooldown_skips_fresh_success() {
        let now = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let cache = UpdateCheckCache {
            schema_version: 1,
            current_version: "0.1.0".to_owned(),
            latest_version: Some("0.1.1".to_owned()),
            checked_at_ms: Some(1_700_000_000_000),
            failed_at_ms: None,
        };
        assert!(!should_refresh(Some(&cache), now + Duration::from_secs(60)));
        assert!(should_refresh(
            Some(&cache),
            now + SUCCESS_COOLDOWN + Duration::from_secs(1)
        ));
    }

    #[test]
    fn reminder_only_on_tty_when_newer() {
        let cache = UpdateCheckCache {
            schema_version: 1,
            current_version: "0.1.0".to_owned(),
            latest_version: Some("0.1.1".to_owned()),
            checked_at_ms: Some(1),
            failed_at_ms: None,
        };
        let mut sink = Vec::new();
        maybe_print_reminder(&cache, "0.1.0", false, &mut sink);
        assert!(sink.is_empty());
        maybe_print_reminder(&cache, "0.1.0", true, &mut sink);
        assert!(
            String::from_utf8(sink)
                .unwrap()
                .contains("Update available: 0.1.1")
        );
    }

    #[test]
    fn no_update_check_skips_reminder_and_spawn() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("update-check.json");
        write_cache(
            &path,
            &UpdateCheckCache {
                schema_version: 1,
                current_version: "0.1.0".to_owned(),
                latest_version: Some("0.1.1".to_owned()),
                checked_at_ms: Some(1),
                failed_at_ms: None,
            },
        )
        .unwrap();
        let mut stderr = Vec::new();
        pre_command_update_check(
            true,
            true,
            "0.1.0",
            &path,
            &temp.path().join("missing-ocd"),
            true,
            &mut stderr,
        );
        assert!(stderr.is_empty());
    }

    #[test]
    fn empty_cache_helpers_and_failure_cooldown() {
        let cache = UpdateCheckCache::empty("0.1.0");
        assert!(!cache.success());
        assert_eq!(cache.current_version, "0.1.0");
        assert!(should_refresh(None, SystemTime::now()));
        let now = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let failed = UpdateCheckCache {
            schema_version: 1,
            current_version: "0.1.0".to_owned(),
            latest_version: None,
            checked_at_ms: None,
            failed_at_ms: Some(1_700_000_000_000),
        };
        assert!(!should_refresh(
            Some(&failed),
            now + Duration::from_secs(60)
        ));
        assert!(should_refresh(
            Some(&failed),
            now + FAILURE_COOLDOWN + Duration::from_secs(1)
        ));
    }

    #[test]
    fn pre_command_prints_reminder_without_network() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("update-check.json");
        write_cache(
            &path,
            &UpdateCheckCache {
                schema_version: 1,
                current_version: "0.1.0".to_owned(),
                latest_version: Some("0.1.1".to_owned()),
                checked_at_ms: Some(1),
                failed_at_ms: None,
            },
        )
        .unwrap();
        let mut stderr = Vec::new();
        pre_command_update_check(
            false,
            false,
            "0.1.0",
            &path,
            &temp.path().join("missing-ocd"),
            true,
            &mut stderr,
        );
        assert!(
            String::from_utf8(stderr)
                .unwrap()
                .contains("Update available: 0.1.1")
        );
    }

    #[test]
    fn spawn_detached_helper_fails_for_missing_exe() {
        let err =
            spawn_detached_helper(Path::new("/tmp/definitely-missing-ocd-binary")).unwrap_err();
        assert_eq!(err.code(), ErrorCode::Internal);
    }

    #[test]
    fn default_cache_path_is_absolute() {
        let path = default_cache_path().unwrap();
        assert!(path.is_absolute());
        assert!(path.ends_with("update-check.json"));
    }

    #[tokio::test]
    async fn run_update_check_helper_writes_success_cache() {
        use crate::install_receipt::{InstallReceipt, RECEIPT_SCHEMA_VERSION, write_receipt};
        use crate::release_upgrade::FixtureReleaseHttp;
        use sha2::{Digest, Sha256};

        let temp = TempDir::new().unwrap();
        let binary = temp.path().join("bin/ocd");
        fs::create_dir_all(binary.parent().unwrap()).unwrap();
        fs::write(&binary, b"ocd").unwrap();
        let receipt_path = temp.path().join("share/open-compute/install-receipt.json");
        write_receipt(
            &receipt_path,
            &InstallReceipt {
                schema_version: RECEIPT_SCHEMA_VERSION,
                version: "0.1.0".to_owned(),
                sha256: "ab".repeat(32),
                target: "darwin-arm64".to_owned(),
                binary_path: binary.to_string_lossy().into_owned(),
                method: "manual".to_owned(),
                source: "test://".to_owned(),
                installed_at_ms: 1,
            },
        )
        .unwrap();
        let http = FixtureReleaseHttp::default();
        let target = match (std::env::consts::OS, std::env::consts::ARCH) {
            ("macos", "aarch64") => "darwin-arm64",
            ("linux", "x86_64") => "linux-x64",
            ("linux", "aarch64") => "linux-arm64",
            _ => "darwin-arm64",
        };
        let tag = "v0.1.9";
        let filename = format!("ocd-{tag}-{target}");
        let binary_bytes = b"next";
        let digest = hex::encode(Sha256::digest(binary_bytes));
        let manifest = serde_json::json!({
            "schemaVersion": 1,
            "tag": tag,
            "version": "0.1.9",
            "gitRevision": "abc",
            "workerdRelease": "1.0.0",
            "workerdLockSha256": "cd".repeat(32),
            "artifacts": [{
                "target": target,
                "os": "darwin",
                "arch": "arm64",
                "filename": filename,
                "bytes": binary_bytes.len(),
                "sha256": digest,
            }]
        });
        let manifest_bytes = serde_json::to_vec_pretty(&manifest).unwrap();
        let sums = format!(
            "{digest}  {filename}\n{}  release.json\n",
            hex::encode(Sha256::digest(&manifest_bytes))
        );
        http.insert(
            format!("{DEFAULT_GITHUB_API_BASE}/repos/elliothux/open-compute/releases/latest"),
            format!(r#"{{"tag_name":"{tag}","prerelease":false,"draft":false}}"#),
        );
        let base = format!("{DEFAULT_RELEASE_DOWNLOAD_BASE}/{tag}");
        http.insert(format!("{base}/release.json"), manifest_bytes);
        http.insert(format!("{base}/SHA256SUMS"), sums.into_bytes());
        let cache_path = temp.path().join("update-check.json");
        run_update_check_helper(&http, &cache_path, "0.1.0", &receipt_path, &binary, target)
            .await
            .unwrap();
        let cache = read_cache(&cache_path).unwrap();
        assert_eq!(cache.latest_version.as_deref(), Some("0.1.9"));
        assert!(cache.success());
    }

    #[tokio::test]
    async fn run_update_check_helper_records_failure_cache() {
        let temp = TempDir::new().unwrap();
        let binary = temp.path().join("bin/ocd");
        fs::create_dir_all(binary.parent().unwrap()).unwrap();
        fs::write(&binary, b"ocd").unwrap();
        let receipt_path = temp.path().join("missing-receipt.json");
        let http = crate::release_upgrade::FixtureReleaseHttp::default();
        let cache_path = temp.path().join("update-check.json");
        run_update_check_helper(
            &http,
            &cache_path,
            "0.1.0",
            &receipt_path,
            &binary,
            "darwin-arm64",
        )
        .await
        .unwrap();
        let cache = read_cache(&cache_path).unwrap();
        assert!(cache.failed_at_ms.is_some());
        assert!(!cache.success());
    }

    #[test]
    fn pre_command_skips_and_spawns_refresh() {
        let temp = TempDir::new().unwrap();
        let cache_path = temp.path().join("update-check.json");
        let exe = temp.path().join("missing-helper");
        let mut stderr = Vec::new();
        pre_command_update_check(true, true, "0.1.0", &cache_path, &exe, true, &mut stderr);
        assert!(stderr.is_empty());

        write_cache(
            &cache_path,
            &UpdateCheckCache {
                schema_version: CACHE_SCHEMA,
                current_version: "0.1.0".to_owned(),
                latest_version: Some("0.2.0".to_owned()),
                checked_at_ms: Some(1),
                failed_at_ms: None,
            },
        )
        .unwrap();
        let mut stderr = Vec::new();
        pre_command_update_check(false, false, "0.1.0", &cache_path, &exe, true, &mut stderr);
        assert!(
            String::from_utf8(stderr)
                .unwrap()
                .contains("Update available")
        );

        let mut stderr = Vec::new();
        pre_command_update_check(false, true, "0.1.0", &cache_path, &exe, false, &mut stderr);
        // refresh attempted against missing exe; reminder suppressed without tty
        assert!(stderr.is_empty());
    }

    #[test]
    fn write_cache_creates_parent_and_rejects_non_file_parent() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("nested/cache/update-check.json");
        write_cache(&path, &UpdateCheckCache::empty("0.1.0")).unwrap();
        assert!(path.is_file());

        let file_as_parent = temp.path().join("file-parent");
        fs::write(&file_as_parent, b"x").unwrap();
        let err = write_cache(
            &file_as_parent.join("update-check.json"),
            &UpdateCheckCache::empty("0.1.0"),
        )
        .unwrap_err();
        assert_eq!(err.code(), ErrorCode::PathInvalid);
    }

    #[test]
    fn read_cache_rejects_symlink_schema_and_versions() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("update-check.json");
        fs::write(&path, b"{}").unwrap();
        let link = temp.path().join("link.json");
        std::os::unix::fs::symlink(&path, &link).unwrap();
        assert!(read_cache(&link).is_none());
        assert!(read_cache(temp.path()).is_none());

        fs::write(&path, vec![b'a'; MAX_CACHE_BYTES + 1]).unwrap();
        assert!(read_cache(&path).is_none());

        let bad_schema = UpdateCheckCache {
            schema_version: 99,
            current_version: "0.1.0".to_owned(),
            latest_version: None,
            checked_at_ms: Some(1),
            failed_at_ms: None,
        };
        assert!(write_cache(&path, &bad_schema).is_err());
        fs::write(
            &path,
            br#"{"schema_version":99,"current_version":"0.1.0","latest_version":null,"checked_at_ms":1,"failed_at_ms":null}"#,
        )
        .unwrap();
        assert!(read_cache(&path).is_none());

        fs::write(
            &path,
            br#"{"schema_version":1,"current_version":"v0.1.0","latest_version":null,"checked_at_ms":1,"failed_at_ms":null}"#,
        )
        .unwrap();
        assert!(read_cache(&path).is_none());

        fs::write(
            &path,
            br#"{"schema_version":1,"current_version":"0.1.0","latest_version":"v0.2.0","checked_at_ms":1,"failed_at_ms":null}"#,
        )
        .unwrap();
        assert!(read_cache(&path).is_none());
    }

    #[test]
    fn write_cache_rejects_missing_parent_component() {
        let err = write_cache(
            Path::new("update-check.json"),
            &UpdateCheckCache::empty("0.1.0"),
        )
        .unwrap_err();
        assert_eq!(err.code(), ErrorCode::PathInvalid);
    }

    #[test]
    fn reminder_and_refresh_edge_cases() {
        let cache = UpdateCheckCache {
            schema_version: 1,
            current_version: "0.1.0".to_owned(),
            latest_version: None,
            checked_at_ms: Some(1),
            failed_at_ms: None,
        };
        let mut sink = Vec::new();
        maybe_print_reminder(&cache, "0.1.0", true, &mut sink);
        assert!(sink.is_empty());
        let same = UpdateCheckCache {
            latest_version: Some("0.1.0".to_owned()),
            ..cache
        };
        maybe_print_reminder(&same, "0.1.0", true, &mut sink);
        assert!(sink.is_empty());
        assert!(!should_refresh(
            Some(&same),
            UNIX_EPOCH - Duration::from_secs(1)
        ));
        assert!(unix_ms(UNIX_EPOCH - Duration::from_secs(1)).is_err());
    }
}
