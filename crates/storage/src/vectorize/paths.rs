//! Secure product-specific Vectorize filesystem layout.

use crate::fs;
use open_compute_core::{AccountId, ErrorCode, PlatformError, ResourceId};
use std::path::{Path, PathBuf};

const DATABASE_FILE: &str = "data.sqlite";
const STAGING_DIR: &str = ".staging";
const TRASH_DIR: &str = ".trash";

/// Canonical Vectorize directories under the platform data root.
#[derive(Clone, Debug)]
pub struct VectorizePaths {
    root: PathBuf,
}

impl VectorizePaths {
    /// Open or create the product root and private operation directories.
    pub fn open(data_root: &Path) -> Result<Self, PlatformError> {
        let root = data_root.join("vectorize");
        fs::create_dir_secure(&root)?;
        fs::validate_contained(data_root, &root)?;
        for child in [STAGING_DIR, TRASH_DIR] {
            fs::create_dir_secure(&root.join(child))?;
            fs::validate_contained(data_root, &root.join(child))?;
        }
        Ok(Self { root })
    }

    /// Product root `<data>/vectorize`.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Canonical relative control locator for one index.
    #[must_use]
    pub fn storage_key(account: AccountId, resource: ResourceId) -> String {
        format!("v1/{account}/{resource}/{DATABASE_FILE}")
    }

    /// Live directory for one index.
    #[must_use]
    pub fn index_dir(&self, account: AccountId, resource: ResourceId) -> PathBuf {
        self.root
            .join(account.to_string())
            .join(resource.to_string())
    }

    /// Live SQLite file for one index.
    #[must_use]
    pub fn index_path(&self, account: AccountId, resource: ResourceId) -> PathBuf {
        self.index_dir(account, resource).join(DATABASE_FILE)
    }

    /// Resolve a catalog locator only when it matches the typed identities exactly.
    pub fn resolve_storage_key(
        &self,
        storage_key: &str,
        account: AccountId,
        resource: ResourceId,
    ) -> Result<PathBuf, PlatformError> {
        if storage_key != Self::storage_key(account, resource) {
            return Err(path_error());
        }
        let account_dir = self.root.join(account.to_string());
        fs::create_dir_secure(&account_dir)?;
        fs::validate_contained(&self.root, &account_dir)?;
        let index_dir = self.index_dir(account, resource);
        fs::validate_contained(&self.root, &index_dir)?;
        let path = self.index_path(account, resource);
        if path.exists() || std::fs::symlink_metadata(&path).is_ok() {
            fs::validate_contained(&self.root, &path)?;
        }
        Ok(path)
    }

    /// Create a unique staging directory for one resource create.
    pub fn create_staging(&self, resource: ResourceId) -> Result<PathBuf, PlatformError> {
        let name = format!("{resource}.{}", uuid::Uuid::now_v7().hyphenated());
        let path = self.root.join(STAGING_DIR).join(name);
        std::fs::create_dir(&path).map_err(|_| path_error())?;
        fs::chmod(&path, 0o700)?;
        fs::validate_owned_dir(&path)?;
        Ok(path)
    }

    /// Atomically publish a verified staging directory.
    pub fn publish_staging(
        &self,
        staging: &Path,
        account: AccountId,
        resource: ResourceId,
    ) -> Result<(), PlatformError> {
        if staging.parent() != Some(self.root.join(STAGING_DIR).as_path()) {
            return Err(path_error());
        }
        fs::validate_owned_dir(staging)?;
        let account_dir = self.root.join(account.to_string());
        fs::create_dir_secure(&account_dir)?;
        let live = self.index_dir(account, resource);
        if live.exists() || std::fs::symlink_metadata(&live).is_ok() {
            return Err(path_error());
        }
        std::fs::rename(staging, &live).map_err(|_| path_error())?;
        fs::fsync_dir(&account_dir)
    }

    /// Move a live index into recoverable quarantine.
    pub fn quarantine(
        &self,
        account: AccountId,
        resource: ResourceId,
    ) -> Result<Option<PathBuf>, PlatformError> {
        let live = self.index_dir(account, resource);
        if !live.exists() {
            return Ok(None);
        }
        fs::validate_owned_dir(&live)?;
        let target = self
            .root
            .join(TRASH_DIR)
            .join(format!("{resource}.{}", uuid::Uuid::now_v7().hyphenated()));
        std::fs::rename(live, &target).map_err(|_| path_error())?;
        fs::fsync_dir(&self.root.join(TRASH_DIR))?;
        Ok(Some(target))
    }

    /// List staging directories for an exact resource identity.
    pub fn staging_candidates(&self, resource: ResourceId) -> Result<Vec<PathBuf>, PlatformError> {
        self.operation_candidates(STAGING_DIR, resource)
    }

    /// List quarantined directories for an exact resource identity.
    pub fn quarantine_candidates(
        &self,
        resource: ResourceId,
    ) -> Result<Vec<PathBuf>, PlatformError> {
        self.operation_candidates(TRASH_DIR, resource)
    }

    /// Remove a validated operation directory containing only SQLite-owned files.
    pub fn remove_operation_dir(&self, path: &Path) -> Result<(), PlatformError> {
        let parent = path.parent().ok_or_else(path_error)?;
        if parent != self.root.join(STAGING_DIR) && parent != self.root.join(TRASH_DIR) {
            return Err(path_error());
        }
        fs::validate_owned_dir(path)?;
        for entry in std::fs::read_dir(path).map_err(|_| path_error())? {
            let entry = entry.map_err(|_| path_error())?;
            let name = entry.file_name();
            let allowed = name.to_str().is_some_and(|name| {
                matches!(name, DATABASE_FILE | "data.sqlite-wal" | "data.sqlite-shm")
            });
            let kind = entry.file_type().map_err(|_| path_error())?;
            if !allowed || !kind.is_file() || kind.is_symlink() {
                return Err(path_error());
            }
            std::fs::remove_file(entry.path()).map_err(|_| path_error())?;
        }
        std::fs::remove_dir(path).map_err(|_| path_error())?;
        fs::fsync_dir(parent)
    }

    fn operation_candidates(
        &self,
        directory: &str,
        resource: ResourceId,
    ) -> Result<Vec<PathBuf>, PlatformError> {
        let parent = self.root.join(directory);
        let prefix = format!("{resource}.");
        let mut candidates = Vec::new();
        for entry in std::fs::read_dir(parent).map_err(|_| path_error())? {
            let entry = entry.map_err(|_| path_error())?;
            if entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.starts_with(&prefix))
            {
                fs::validate_owned_dir(&entry.path())?;
                candidates.push(entry.path());
            }
        }
        candidates.sort();
        Ok(candidates)
    }
}

fn path_error() -> PlatformError {
    PlatformError::new(
        ErrorCode::PathInvalid,
        "Vectorize resource path invariant failed",
    )
}
