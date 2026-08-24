//! Typed, account-scoped KV filesystem layout.

use crate::fs;
use open_compute_core::{AccountId, ErrorCode, PlatformError, ResourceId};
use std::path::{Path, PathBuf};

const KV_DIR: &str = "kv";
const STAGING_DIR: &str = ".staging";
const WRITE_STAGING_DIR: &str = ".staging-write";
const TRASH_DIR: &str = ".trash";
const DATABASE_FILE: &str = "data.sqlite";

/// Canonical KV physical paths rooted in the owned platform data directory.
#[derive(Clone, Debug)]
pub struct KvPaths {
    root: PathBuf,
}

impl KvPaths {
    /// Create and validate the product-owned KV layout on first use.
    pub fn open(data_root: &Path) -> Result<Self, PlatformError> {
        fs::require_absolute(data_root)?;
        fs::validate_root(data_root)?;
        let root = data_root.join(KV_DIR);
        fs::create_dir_secure(&root)?;
        for child in [STAGING_DIR, WRITE_STAGING_DIR, TRASH_DIR] {
            fs::create_dir_secure(&root.join(child))?;
            fs::validate_contained(data_root, &root.join(child))?;
        }
        Ok(Self { root })
    }

    /// Product root `<data>/kv`.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Canonical relative control locator for one namespace database.
    #[must_use]
    pub fn storage_key(account: AccountId, resource: ResourceId) -> String {
        format!("v1/{account}/{resource}/{DATABASE_FILE}")
    }

    /// Resolve and validate a canonical product locator against typed identity.
    pub fn resolve_storage_key(
        &self,
        storage_key: &str,
        account: AccountId,
        resource: ResourceId,
    ) -> Result<PathBuf, PlatformError> {
        if storage_key != Self::storage_key(account, resource) {
            return Err(invariant());
        }
        let account_dir = self.ensure_account_dir(account)?;
        fs::validate_contained(&self.root, &account_dir)?;
        let namespace = self.namespace_dir(account, resource);
        fs::validate_contained(&self.root, &namespace)?;
        let path = self.database_path(account, resource);
        if path.exists() || std::fs::symlink_metadata(&path).is_ok() {
            fs::validate_contained(&self.root, &path)?;
        }
        Ok(path)
    }

    /// Live namespace directory.
    #[must_use]
    pub fn namespace_dir(&self, account: AccountId, resource: ResourceId) -> PathBuf {
        self.root
            .join(account.to_string())
            .join(resource.to_string())
    }

    /// Live namespace SQLite database.
    #[must_use]
    pub fn database_path(&self, account: AccountId, resource: ResourceId) -> PathBuf {
        self.namespace_dir(account, resource).join(DATABASE_FILE)
    }

    /// Create and validate the account directory.
    pub fn ensure_account_dir(&self, account: AccountId) -> Result<PathBuf, PlatformError> {
        let path = self.root.join(account.to_string());
        fs::create_dir_secure(&path)?;
        fs::validate_contained(&self.root, &path)?;
        Ok(path)
    }

    /// Create a unique namespace-create staging directory.
    pub fn create_namespace_staging(&self, resource: ResourceId) -> Result<PathBuf, PlatformError> {
        let name = format!("{resource}.{}", uuid::Uuid::now_v7().hyphenated());
        let path = self.root.join(STAGING_DIR).join(name);
        std::fs::create_dir(&path).map_err(|_| {
            PlatformError::new(
                ErrorCode::PathInvalid,
                "failed to create KV staging directory",
            )
        })?;
        fs::chmod(&path, 0o700)?;
        fs::validate_owned_dir(&path)?;
        Ok(path)
    }

    /// Publish a verified staging directory by same-filesystem atomic rename.
    pub fn publish_staging(
        &self,
        staging: &Path,
        account: AccountId,
        resource: ResourceId,
    ) -> Result<(), PlatformError> {
        fs::validate_owned_dir(staging)?;
        let account_dir = self.ensure_account_dir(account)?;
        let live = self.namespace_dir(account, resource);
        if live.exists() || std::fs::symlink_metadata(&live).is_ok() {
            return Err(invariant());
        }
        std::fs::rename(staging, &live).map_err(|_| {
            PlatformError::new(ErrorCode::PathInvalid, "failed to publish KV namespace")
        })?;
        fs::fsync_dir(&account_dir)
    }

    /// Move one exact live namespace directory into recoverable quarantine.
    pub fn quarantine(
        &self,
        account: AccountId,
        resource: ResourceId,
    ) -> Result<Option<PathBuf>, PlatformError> {
        let live = self.namespace_dir(account, resource);
        if !live.exists() {
            return Ok(None);
        }
        fs::validate_owned_dir(&live)?;
        let name = format!("{resource}.{}", uuid::Uuid::now_v7().hyphenated());
        let trash = self.root.join(TRASH_DIR).join(name);
        std::fs::rename(&live, &trash).map_err(|_| {
            PlatformError::new(ErrorCode::PathInvalid, "failed to quarantine KV namespace")
        })?;
        fs::fsync_dir(&self.root.join(TRASH_DIR))?;
        Ok(Some(trash))
    }

    /// Remove one already-validated quarantine directory without following links.
    pub fn remove_quarantine(&self, path: &Path) -> Result<(), PlatformError> {
        let parent = path.parent().ok_or_else(invariant)?;
        if parent != self.root.join(TRASH_DIR) {
            return Err(invariant());
        }
        fs::validate_owned_dir(path)?;
        for entry in std::fs::read_dir(path).map_err(|_| invariant())? {
            let entry = entry.map_err(|_| invariant())?;
            let kind = entry.file_type().map_err(|_| invariant())?;
            if kind.is_symlink() || !kind.is_file() {
                return Err(invariant());
            }
            std::fs::remove_file(entry.path()).map_err(|_| invariant())?;
        }
        std::fs::remove_dir(path).map_err(|_| invariant())?;
        fs::fsync_dir(parent)
    }

    /// Enumerate only staging directories whose names prove the exact resource prefix and UUID token.
    pub fn namespace_staging_candidates(
        &self,
        resource: ResourceId,
    ) -> Result<Vec<PathBuf>, PlatformError> {
        self.operation_candidates(STAGING_DIR, resource)
    }

    /// Enumerate only quarantines whose names prove the exact resource prefix and UUID token.
    pub fn quarantine_candidates(
        &self,
        resource: ResourceId,
    ) -> Result<Vec<PathBuf>, PlatformError> {
        self.operation_candidates(TRASH_DIR, resource)
    }

    /// Remove one exact create-staging directory after validating every contained entry.
    pub fn remove_namespace_staging(&self, path: &Path) -> Result<(), PlatformError> {
        if path.parent() != Some(self.root.join(STAGING_DIR).as_path()) {
            return Err(invariant());
        }
        self.remove_owned_namespace_dir(path)
    }

    /// Secure per-request value staging path, never derived from a tenant key.
    pub fn create_write_staging(
        &self,
        resource: ResourceId,
        request_id: &str,
    ) -> Result<PathBuf, PlatformError> {
        let request = uuid::Uuid::parse_str(request_id).map_err(|_| invariant())?;
        if request.hyphenated().to_string() != request_id {
            return Err(invariant());
        }
        let resource_dir = self.root.join(WRITE_STAGING_DIR).join(resource.to_string());
        fs::create_dir_secure(&resource_dir)?;
        let path = resource_dir.join(request_id);
        let file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .map_err(|_| PlatformError::new(ErrorCode::PathInvalid, "failed to stage KV value"))?;
        file.sync_all().map_err(|_| {
            PlatformError::new(ErrorCode::PathInvalid, "failed to sync KV staging file")
        })?;
        drop(file);
        fs::validate_owned_file(&path, true)?;
        Ok(path)
    }

    /// Remove only canonical request staging files during single-owner startup.
    ///
    /// Callers must invoke this before serving requests, when no live staging
    /// writer can exist in the current process generation.
    pub fn cleanup_write_staging(&self) -> Result<u32, PlatformError> {
        let root = self.root.join(WRITE_STAGING_DIR);
        let mut removed = 0_u32;
        for resource_entry in std::fs::read_dir(&root).map_err(|_| invariant())? {
            let resource_entry = resource_entry.map_err(|_| invariant())?;
            let resource_name = resource_entry.file_name();
            let Some(resource_name) = resource_name.to_str() else {
                continue;
            };
            if uuid::Uuid::parse_str(resource_name)
                .ok()
                .is_none_or(|id| id.hyphenated().to_string() != resource_name)
            {
                continue;
            }
            let kind = resource_entry.file_type().map_err(|_| invariant())?;
            if kind.is_symlink() || !kind.is_dir() {
                return Err(invariant());
            }
            let directory = resource_entry.path();
            let mut unknown = false;
            for request_entry in std::fs::read_dir(&directory).map_err(|_| invariant())? {
                let request_entry = request_entry.map_err(|_| invariant())?;
                let request_name = request_entry.file_name();
                let canonical = request_name.to_str().is_some_and(|name| {
                    uuid::Uuid::parse_str(name)
                        .ok()
                        .is_some_and(|id| id.hyphenated().to_string() == name)
                });
                let kind = request_entry.file_type().map_err(|_| invariant())?;
                if !canonical || kind.is_symlink() || !kind.is_file() {
                    unknown = true;
                    continue;
                }
                std::fs::remove_file(request_entry.path()).map_err(|_| invariant())?;
                removed = removed.saturating_add(1);
            }
            if !unknown {
                std::fs::remove_dir(&directory).map_err(|_| invariant())?;
            }
        }
        fs::fsync_dir(&root)?;
        Ok(removed)
    }

    fn operation_candidates(
        &self,
        directory: &str,
        resource: ResourceId,
    ) -> Result<Vec<PathBuf>, PlatformError> {
        let parent = self.root.join(directory);
        let prefix = format!("{resource}.");
        let mut candidates = Vec::new();
        for entry in std::fs::read_dir(&parent).map_err(|_| invariant())? {
            let entry = entry.map_err(|_| invariant())?;
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            let Some(token) = name.strip_prefix(&prefix) else {
                continue;
            };
            if uuid::Uuid::parse_str(token)
                .ok()
                .is_none_or(|id| id.hyphenated().to_string() != token)
            {
                continue;
            }
            let kind = entry.file_type().map_err(|_| invariant())?;
            if kind.is_symlink() || !kind.is_dir() {
                return Err(invariant());
            }
            candidates.push(entry.path());
        }
        candidates.sort();
        Ok(candidates)
    }

    fn remove_owned_namespace_dir(&self, path: &Path) -> Result<(), PlatformError> {
        fs::validate_owned_dir(path)?;
        for entry in std::fs::read_dir(path).map_err(|_| invariant())? {
            let entry = entry.map_err(|_| invariant())?;
            let kind = entry.file_type().map_err(|_| invariant())?;
            if kind.is_symlink() || !kind.is_file() {
                return Err(invariant());
            }
            std::fs::remove_file(entry.path()).map_err(|_| invariant())?;
        }
        let parent = path.parent().ok_or_else(invariant)?;
        std::fs::remove_dir(path).map_err(|_| invariant())?;
        fs::fsync_dir(parent)
    }
}

fn invariant() -> PlatformError {
    PlatformError::new(
        ErrorCode::ResourceInvariantViolation,
        "KV physical identity invariant failed",
    )
}

use std::os::unix::fs::OpenOptionsExt as _;

#[cfg(test)]
#[path = "paths_tests.rs"]
mod tests;
