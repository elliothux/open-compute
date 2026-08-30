//! Secure account/Worker response-cache filesystem layout.

use crate::fs;
use open_compute_core::{AccountId, ErrorCode, PlatformError, WorkerId};
use std::path::{Path, PathBuf};

const CACHE_DIR: &str = "cache";
const ARTIFACT_CACHE_DIR: &str = "artifacts";
const DATABASE_FILE: &str = "cache.sqlite";

/// Canonical response-cache paths rooted below the owned data directory.
#[derive(Clone, Debug)]
pub struct CachePaths {
    root: PathBuf,
}

impl CachePaths {
    /// Validate the data root and cache parent without creating tenant paths.
    pub fn open(data_root: &Path) -> Result<Self, PlatformError> {
        fs::require_absolute(data_root)?;
        fs::validate_root(data_root)?;
        let root = data_root.join(CACHE_DIR);
        fs::validate_owned_dir(&root)?;
        fs::validate_contained(data_root, &root)?;
        Ok(Self { root })
    }

    /// Response-cache root shared with the separate `artifacts/` local cache sibling.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Canonical per-Worker database path.
    #[must_use]
    pub fn database_path(&self, account: AccountId, worker: WorkerId) -> PathBuf {
        self.root
            .join(account.to_string())
            .join(worker.to_string())
            .join(DATABASE_FILE)
    }

    /// Create or validate the account and Worker directories.
    pub fn ensure_worker_dir(
        &self,
        account: AccountId,
        worker: WorkerId,
    ) -> Result<PathBuf, PlatformError> {
        let account_dir = self.root.join(account.to_string());
        fs::create_dir_secure(&account_dir)?;
        fs::validate_contained(&self.root, &account_dir)?;
        let worker_dir = account_dir.join(worker.to_string());
        fs::create_dir_secure(&worker_dir)?;
        fs::validate_contained(&self.root, &worker_dir)?;
        Ok(worker_dir)
    }

    /// Enumerate canonical existing per-Worker databases without following links.
    pub fn databases(&self) -> Result<Vec<PathBuf>, PlatformError> {
        let mut databases = Vec::new();
        for account in std::fs::read_dir(&self.root).map_err(|_| unavailable())? {
            let account = account.map_err(|_| unavailable())?;
            let name = account.file_name();
            if name == ARTIFACT_CACHE_DIR {
                continue;
            }
            let Some(name) = name.to_str() else { continue };
            if name.parse::<AccountId>().is_err() {
                continue;
            }
            if account.file_type().map_err(|_| unavailable())?.is_symlink() {
                return Err(corrupt_path());
            }
            if !account.file_type().map_err(|_| unavailable())?.is_dir() {
                continue;
            }
            fs::validate_owned_dir(&account.path())?;
            for worker in std::fs::read_dir(account.path()).map_err(|_| unavailable())? {
                let worker = worker.map_err(|_| unavailable())?;
                let Some(name) = worker.file_name().to_str().map(str::to_owned) else {
                    continue;
                };
                if name.parse::<WorkerId>().is_err() {
                    continue;
                }
                let kind = worker.file_type().map_err(|_| unavailable())?;
                if kind.is_symlink() {
                    return Err(corrupt_path());
                }
                if !kind.is_dir() {
                    continue;
                }
                fs::validate_owned_dir(&worker.path())?;
                let database = worker.path().join(DATABASE_FILE);
                if database.exists() || std::fs::symlink_metadata(&database).is_ok() {
                    fs::validate_owned_file(&database, true)?;
                    databases.push(database);
                }
            }
        }
        databases.sort();
        Ok(databases)
    }
}

fn unavailable() -> PlatformError {
    PlatformError::new(
        ErrorCode::CacheUnavailable,
        "cache filesystem is unavailable",
    )
}

fn corrupt_path() -> PlatformError {
    PlatformError::new(
        ErrorCode::CacheCorrupt,
        "cache filesystem identity is invalid",
    )
}
