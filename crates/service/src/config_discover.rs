//! Deterministic bootstrap config discovery for operator commands.

use crate::config_load::{LoadedConfig, lexical_absolute, load_platform_config_from};
use open_compute_core::{ErrorCode, PlatformError};
use std::path::{Path, PathBuf};

/// Product config filename discovered in the startup working directory.
pub const PROJECT_CONFIG_NAME: &str = "compute.toml";

/// System-wide default config path.
pub const SYSTEM_CONFIG_PATH: &str = "/etc/open-compute/config.toml";

/// How a configuration path was selected.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConfigDiscoverySource {
    /// Explicit `--config`.
    Explicit,
    /// Exact `./compute.toml` in the startup working directory.
    Project,
    /// `/etc/open-compute/config.toml`.
    System,
}

/// Result of config path discovery before or after loading.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiscoveredConfigPath {
    /// Candidate path as resolved against the startup working directory for relative inputs.
    pub path: PathBuf,
    /// Discovery rule that selected the path.
    pub source: ConfigDiscoverySource,
}

/// Resolve the config path using the P11 priority rules without loading TOML.
///
/// Existence is checked with `symlink_metadata`. A present but unloadable file is
/// still returned so the subsequent load can surface the exact failure without
/// falling back to a lower-priority path.
pub fn discover_config_path(
    explicit: Option<&Path>,
    startup_cwd: &Path,
) -> Result<DiscoveredConfigPath, PlatformError> {
    if let Some(path) = explicit {
        let absolute = lexical_absolute(startup_cwd, path)?;
        return Ok(DiscoveredConfigPath {
            path: absolute,
            source: ConfigDiscoverySource::Explicit,
        });
    }

    let project = startup_cwd.join(PROJECT_CONFIG_NAME);
    if path_exists_nofollow(&project)? {
        return Ok(DiscoveredConfigPath {
            path: project,
            source: ConfigDiscoverySource::Project,
        });
    }

    let system = PathBuf::from(SYSTEM_CONFIG_PATH);
    if path_exists_nofollow(&system)? {
        return Ok(DiscoveredConfigPath {
            path: system,
            source: ConfigDiscoverySource::System,
        });
    }

    Err(PlatformError::new(
        ErrorCode::ConfigPathInvalid,
        "no configuration found; checked ./compute.toml and /etc/open-compute/config.toml; run `ocd setup`",
    ))
}

/// Discover and load the platform configuration.
pub fn discover_and_load_config(
    explicit: Option<&Path>,
    startup_cwd: &Path,
) -> Result<LoadedConfig, PlatformError> {
    let discovered = discover_config_path(explicit, startup_cwd)?;
    load_platform_config_from(&discovered.path, startup_cwd)
}

fn path_exists_nofollow(path: &Path) -> Result<bool, PlatformError> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(_) => Err(PlatformError::new(
            ErrorCode::ConfigPathInvalid,
            "failed to inspect a candidate configuration path",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::atomic::{AtomicU64, Ordering};
    use uuid::Uuid;

    fn scratch() -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        // Unique top-level directory: shared parents under TMPDIR fail Gate cleanup.
        let dir = std::env::temp_dir().join(format!(
            "open-compute-config-discover-{}-{}",
            Uuid::now_v7().as_hyphenated(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn explicit_beats_project_and_system() {
        let dir = scratch();
        let project = dir.join(PROJECT_CONFIG_NAME);
        fs::write(&project, "not-used").unwrap();
        let explicit = dir.join("explicit.toml");
        fs::write(&explicit, "not-loaded-here").unwrap();
        let found = discover_config_path(Some(&explicit), &dir).unwrap();
        assert_eq!(found.source, ConfigDiscoverySource::Explicit);
        assert_eq!(found.path, lexical_absolute(&dir, &explicit).unwrap());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn project_beats_missing_explicit() {
        let dir = scratch();
        let project = dir.join(PROJECT_CONFIG_NAME);
        fs::write(&project, "x = 1\n").unwrap();
        let found = discover_config_path(None, &dir).unwrap();
        assert_eq!(found.source, ConfigDiscoverySource::Project);
        assert_eq!(found.path, project);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn missing_everything_lists_checked_paths() {
        let dir = scratch();
        let err = discover_config_path(None, &dir).unwrap_err();
        assert_eq!(err.code(), ErrorCode::ConfigPathInvalid);
        assert!(err.message().contains("compute.toml"));
        assert!(err.message().contains("/etc/open-compute/config.toml"));
        assert!(err.message().contains("ocd setup"));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn present_corrupt_project_is_selected_without_fallback() {
        let dir = scratch();
        let project = dir.join(PROJECT_CONFIG_NAME);
        fs::write(&project, "this is not toml [[[").unwrap();
        let found = discover_config_path(None, &dir).unwrap();
        assert_eq!(found.source, ConfigDiscoverySource::Project);
        let err = discover_and_load_config(None, &dir).unwrap_err();
        assert!(matches!(
            err.code(),
            ErrorCode::ConfigParseFailed | ErrorCode::ConfigInvalid
        ));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn path_exists_fails_closed_on_permission_errors() {
        let dir = scratch();
        let blocked = dir.join("blocked");
        fs::create_dir_all(&blocked).unwrap();
        let nested = blocked.join("compute.toml");
        fs::write(&nested, "x = 1\n").unwrap();
        fs::set_permissions(&blocked, fs::Permissions::from_mode(0o000)).unwrap();
        let err = path_exists_nofollow(&nested);
        let _ = fs::set_permissions(&blocked, fs::Permissions::from_mode(0o755));
        assert!(err.is_err());
        let _ = fs::remove_dir_all(dir);
    }
}
