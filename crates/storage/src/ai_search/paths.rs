//! Secure product-specific AI Search filesystem layout.

use crate::fs;
use open_compute_core::{AccountId, ErrorCode, PlatformError, ResourceId};
use std::path::{Path, PathBuf};

const DATABASE_FILE: &str = "data.sqlite";
const STAGING_DIR: &str = ".staging";
const TRASH_DIR: &str = ".trash";

/// Canonical AI Search directories under the platform data root.
#[derive(Clone, Debug)]
pub struct AiSearchPaths {
    root: PathBuf,
}

impl AiSearchPaths {
    /// Open or create the product root and private operation directories.
    pub fn open(data_root: &Path) -> Result<Self, PlatformError> {
        let root = data_root.join("ai-search");
        fs::create_dir_secure(&root)?;
        fs::validate_contained(data_root, &root)?;
        for child in [STAGING_DIR, TRASH_DIR] {
            fs::create_dir_secure(&root.join(child))?;
            fs::validate_contained(data_root, &root.join(child))?;
        }
        Ok(Self { root })
    }

    /// Product root `<data>/ai-search`.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Canonical relative control locator for one instance.
    #[must_use]
    pub fn storage_key(account: AccountId, resource: ResourceId) -> String {
        format!("v1/{account}/{resource}/{DATABASE_FILE}")
    }

    /// Live directory for one instance.
    #[must_use]
    pub fn instance_dir(&self, account: AccountId, resource: ResourceId) -> PathBuf {
        self.root
            .join(account.to_string())
            .join(resource.to_string())
    }

    /// Live SQLite authority path for one instance.
    #[must_use]
    pub fn instance_path(&self, account: AccountId, resource: ResourceId) -> PathBuf {
        self.instance_dir(account, resource).join(DATABASE_FILE)
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
        let instance_dir = self.instance_dir(account, resource);
        fs::validate_contained(&self.root, &instance_dir)?;
        let path = self.instance_path(account, resource);
        if path.exists() || std::fs::symlink_metadata(&path).is_ok() {
            fs::validate_contained(&self.root, &path)?;
        }
        Ok(path)
    }

    /// Create a unique private staging directory for one instance create.
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
        let live = self.instance_dir(account, resource);
        if live.exists() || std::fs::symlink_metadata(&live).is_ok() {
            return Err(path_error());
        }
        std::fs::rename(staging, &live).map_err(|_| path_error())?;
        fs::fsync_dir(&account_dir)
    }

    /// Move a live instance into recoverable quarantine.
    pub fn quarantine(
        &self,
        account: AccountId,
        resource: ResourceId,
    ) -> Result<Option<PathBuf>, PlatformError> {
        let live = self.instance_dir(account, resource);
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

    /// Enumerate private create staging directories for one resource.
    pub fn staging_candidates(&self, resource: ResourceId) -> Result<Vec<PathBuf>, PlatformError> {
        self.operation_candidates(STAGING_DIR, resource)
    }

    /// Enumerate private delete quarantine directories for one resource.
    pub fn quarantine_candidates(
        &self,
        resource: ResourceId,
    ) -> Result<Vec<PathBuf>, PlatformError> {
        self.operation_candidates(TRASH_DIR, resource)
    }

    /// Remove one exact product-owned staging or quarantine directory.
    pub fn remove_operation_dir(&self, path: &Path) -> Result<(), PlatformError> {
        let parent = path.parent().ok_or_else(path_error)?;
        if parent != self.root.join(STAGING_DIR) && parent != self.root.join(TRASH_DIR) {
            return Err(path_error());
        }
        fs::validate_owned_dir(path)?;
        std::fs::remove_dir_all(path).map_err(|_| path_error())?;
        fs::fsync_dir(parent)
    }

    fn operation_candidates(
        &self,
        directory: &str,
        resource: ResourceId,
    ) -> Result<Vec<PathBuf>, PlatformError> {
        let parent = self.root.join(directory);
        fs::validate_owned_dir(&parent)?;
        let prefix = format!("{resource}.");
        let mut paths = Vec::new();
        for entry in std::fs::read_dir(&parent).map_err(|_| path_error())? {
            let entry = entry.map_err(|_| path_error())?;
            let file_type = entry.file_type().map_err(|_| path_error())?;
            let name = entry.file_name();
            let name = name.to_str().ok_or_else(path_error)?;
            if name.starts_with(&prefix) {
                if !file_type.is_dir() || file_type.is_symlink() {
                    return Err(path_error());
                }
                let path = entry.path();
                fs::validate_owned_dir(&path)?;
                paths.push(path);
            }
        }
        paths.sort();
        Ok(paths)
    }
}

fn path_error() -> PlatformError {
    PlatformError::new(
        ErrorCode::PathInvalid,
        "AI Search resource path invariant failed",
    )
}
