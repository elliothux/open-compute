//! Data directory acquisition and layout.

use crate::fs;
use crate::lock::{DataDirLock, FilesystemDurability};
use open_compute_core::{PlatformError, StartupId, config::StorageConfig};
use std::path::{Path, PathBuf};

const KEYS: &str = "keys";
const RUNTIME: &str = "runtime";
const RUNTIME_PREVIOUS: &str = "previous";
const CACHE: &str = "cache";
const ARTIFACTS: &str = "artifacts";
const SHA256: &str = "sha256";
const DEPLOYMENT_STAGING: &str = "deployment-staging";
const BACKUP_STAGING: &str = "backup-staging";
const DIAGNOSTICS: &str = "diagnostics";
const FAILED_STARTS: &str = "failed-starts";
const LOCK_NAME: &str = "platform.lock";
const CONTROL_DB_NAME: &str = "control.sqlite";

/// P0.1 layout names that must not be pre-created as tenant resource files.
pub const FORBIDDEN_PRECREATE: &[&str] = &["do", "kv", "d1", "scheduler.sqlite"];

/// RAII owner of a data directory and its exclusive lock.
#[derive(Debug)]
pub struct DataDir {
    root: PathBuf,
    lock: DataDirLock,
}

impl DataDir {
    /// Acquire exclusive ownership of `config.storage.data_dir`.
    pub fn acquire(config: &StorageConfig) -> Result<Self, PlatformError> {
        let root = &config.data_dir;
        fs::require_absolute(root)?;
        if root.exists() {
            fs::validate_root(root)?;
        } else {
            fs::create_root_first_run(root)?;
        }
        create_layout(root)?;
        let lock_path = config.data_lock_path();
        fs::validate_contained(root, &lock_path)?;
        let lock = DataDirLock::acquire(&lock_path, StartupId::generate())?;
        let data_dir = Self {
            root: root.clone(),
            lock,
        };
        data_dir.validate_children()?;
        data_dir.clear_deployment_staging()?;
        Ok(data_dir)
    }

    /// Absolute data root.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Control database path: `<data_dir>/control.sqlite`.
    #[must_use]
    pub fn control_db_path(&self) -> PathBuf {
        self.root.join(CONTROL_DB_NAME)
    }

    /// Keys directory.
    #[must_use]
    pub fn keys_dir(&self) -> PathBuf {
        self.root.join(KEYS)
    }

    /// Runtime compile-cache directory.
    #[must_use]
    pub fn runtime_dir(&self) -> PathBuf {
        self.root.join(RUNTIME)
    }

    /// Artifact cache directory: `<data>/cache/artifacts`.
    #[must_use]
    pub fn artifact_cache_dir(&self) -> PathBuf {
        self.root.join(CACHE).join(ARTIFACTS)
    }

    /// Private crash-recoverable staging directory for streamed deployment uploads.
    #[must_use]
    pub fn deployment_staging_dir(&self) -> PathBuf {
        self.root.join(DEPLOYMENT_STAGING)
    }

    /// Held lock.
    #[must_use]
    pub fn lock(&self) -> &DataDirLock {
        &self.lock
    }

    /// Filesystem durability hint for doctor.
    #[must_use]
    pub fn filesystem_durability(&self) -> FilesystemDurability {
        self.lock.filesystem_durability()
    }

    pub(crate) fn record_platform_id(&self, platform_id: &str) -> Result<(), PlatformError> {
        self.lock.write_metadata(Some(platform_id))
    }

    /// Create `control.sqlite` as a 0600 regular file after the master key is resolved.
    pub(crate) fn ensure_control_db(&self) -> Result<PathBuf, PlatformError> {
        let db_path = self.control_db_path();
        fs::validate_contained(&self.root, &db_path)?;
        fs::ensure_file_secure(&db_path)?;
        fs::validate_contained(&self.root, &db_path)?;
        Ok(db_path)
    }

    fn validate_children(&self) -> Result<(), PlatformError> {
        for rel in [
            LOCK_NAME,
            CONTROL_DB_NAME,
            KEYS,
            RUNTIME,
            CACHE,
            DEPLOYMENT_STAGING,
            BACKUP_STAGING,
            DIAGNOSTICS,
        ] {
            let child = self.root.join(rel);
            fs::validate_contained(&self.root, &child)?;
        }
        fs::validate_owned_file(&self.root.join(LOCK_NAME), true)?;
        let db_path = self.root.join(CONTROL_DB_NAME);
        if db_path.exists() || std::fs::symlink_metadata(&db_path).is_ok() {
            fs::validate_owned_file(&db_path, true)?;
        }
        for dir in [
            self.keys_dir(),
            self.root.join(RUNTIME),
            self.root.join(RUNTIME).join(RUNTIME_PREVIOUS),
            self.root.join(CACHE),
            self.root.join(CACHE).join(ARTIFACTS),
            self.root.join(CACHE).join(ARTIFACTS).join(SHA256),
            self.deployment_staging_dir(),
            self.root.join(BACKUP_STAGING),
            self.root.join(DIAGNOSTICS),
            self.root.join(DIAGNOSTICS).join(FAILED_STARTS),
        ] {
            fs::validate_owned_dir(&dir)?;
            fs::validate_contained(&self.root, &dir)?;
        }
        Ok(())
    }

    fn clear_deployment_staging(&self) -> Result<(), PlatformError> {
        let staging = self.deployment_staging_dir();
        for entry in std::fs::read_dir(&staging).map_err(|_| {
            PlatformError::new(
                open_compute_core::ErrorCode::PathInvalid,
                "failed to inspect deployment staging directory",
            )
        })? {
            let entry = entry.map_err(|_| {
                PlatformError::new(
                    open_compute_core::ErrorCode::PathInvalid,
                    "failed to inspect deployment staging entry",
                )
            })?;
            let kind = entry.file_type().map_err(|_| {
                PlatformError::new(
                    open_compute_core::ErrorCode::PathInvalid,
                    "failed to inspect deployment staging entry type",
                )
            })?;
            if !kind.is_file() || kind.is_symlink() {
                return Err(PlatformError::new(
                    open_compute_core::ErrorCode::PathInvalid,
                    "deployment staging contains a non-regular entry",
                ));
            }
            std::fs::remove_file(entry.path()).map_err(|_| {
                PlatformError::new(
                    open_compute_core::ErrorCode::PathInvalid,
                    "failed to clear stale deployment staging file",
                )
            })?;
        }
        Ok(())
    }
}

fn create_layout(root: &Path) -> Result<(), PlatformError> {
    fs::create_dir_secure(&root.join(KEYS))?;
    fs::create_dir_secure(&root.join(RUNTIME))?;
    fs::create_dir_secure(&root.join(RUNTIME).join(RUNTIME_PREVIOUS))?;
    fs::create_dir_secure(&root.join(CACHE))?;
    fs::create_dir_secure(&root.join(CACHE).join(ARTIFACTS))?;
    fs::create_dir_secure(&root.join(CACHE).join(ARTIFACTS).join(SHA256))?;
    fs::create_dir_secure(&root.join(DEPLOYMENT_STAGING))?;
    fs::create_dir_secure(&root.join(BACKUP_STAGING))?;
    fs::create_dir_secure(&root.join(DIAGNOSTICS))?;
    fs::create_dir_secure(&root.join(DIAGNOSTICS).join(FAILED_STARTS))?;
    Ok(())
}

/// Paths that must not exist after a clean P0.1 bootstrap.
#[must_use]
pub fn future_resource_paths(root: &Path) -> Vec<PathBuf> {
    FORBIDDEN_PRECREATE
        .iter()
        .map(|name| root.join(name))
        .collect()
}

/// Layout directories created for P0.1.
#[must_use]
pub fn expected_directories(root: &Path) -> Vec<PathBuf> {
    vec![
        root.join(KEYS),
        root.join(RUNTIME),
        root.join(RUNTIME).join(RUNTIME_PREVIOUS),
        root.join(CACHE),
        root.join(CACHE).join(ARTIFACTS),
        root.join(CACHE).join(ARTIFACTS).join(SHA256),
        root.join(DEPLOYMENT_STAGING),
        root.join(BACKUP_STAGING),
        root.join(DIAGNOSTICS),
        root.join(DIAGNOSTICS).join(FAILED_STARTS),
    ]
}
