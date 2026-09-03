//! Secure product-specific D1 filesystem layout.

use crate::fs;
use open_compute_core::{AccountId, ErrorCode, PlatformError, ResourceId};
use std::path::{Path, PathBuf};

const DATABASE_FILE: &str = "data.sqlite";
const HISTORY_DIR: &str = "history";
const STAGING_DIR: &str = ".staging";
const TRASH_DIR: &str = ".trash";

/// Canonical D1 directories under the platform data root.
#[derive(Clone, Debug)]
pub struct D1Paths {
    root: PathBuf,
}

impl D1Paths {
    /// Open or create the product root and its private operation directories.
    pub fn open(data_root: &Path) -> Result<Self, PlatformError> {
        let root = data_root.join("d1");
        fs::create_dir_secure(&root)?;
        fs::validate_contained(data_root, &root)?;
        for child in [STAGING_DIR, TRASH_DIR] {
            fs::create_dir_secure(&root.join(child))?;
            fs::validate_contained(data_root, &root.join(child))?;
        }
        Ok(Self { root })
    }

    /// Product root `<data>/d1`.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Canonical relative control locator for one database.
    #[must_use]
    pub fn storage_key(account: AccountId, resource: ResourceId) -> String {
        format!("v1/{account}/{resource}/{DATABASE_FILE}")
    }

    /// Resolve a catalog locator only when it matches the typed identities exactly.
    pub fn resolve_storage_key(
        &self,
        storage_key: &str,
        account: AccountId,
        resource: ResourceId,
    ) -> Result<PathBuf, PlatformError> {
        if storage_key != Self::storage_key(account, resource) {
            return Err(identity_mismatch());
        }
        let account_dir = self.ensure_account_dir(account)?;
        fs::validate_contained(&self.root, &account_dir)?;
        let database_dir = self.database_dir(account, resource);
        fs::validate_contained(&self.root, &database_dir)?;
        let path = database_dir.join(DATABASE_FILE);
        if path.exists() || std::fs::symlink_metadata(&path).is_ok() {
            fs::validate_contained(&self.root, &path)?;
        }
        Ok(path)
    }

    /// Live directory for one D1 database.
    #[must_use]
    pub fn database_dir(&self, account: AccountId, resource: ResourceId) -> PathBuf {
        self.root
            .join(account.to_string())
            .join(resource.to_string())
    }

    /// Live SQLite file for one D1 database.
    #[must_use]
    pub fn database_path(&self, account: AccountId, resource: ResourceId) -> PathBuf {
        self.database_dir(account, resource).join(DATABASE_FILE)
    }

    /// Canonical private locator for one completed database snapshot.
    #[must_use]
    pub fn snapshot_key(account: AccountId, resource: ResourceId, session_version: u64) -> String {
        format!("v1/{account}/{resource}/{HISTORY_DIR}/{session_version}.sqlite")
    }

    /// Resolve one exact completed snapshot locator without accepting aliases.
    pub fn resolve_snapshot_key(
        &self,
        snapshot_key: &str,
        account: AccountId,
        resource: ResourceId,
        session_version: u64,
    ) -> Result<PathBuf, PlatformError> {
        if snapshot_key != Self::snapshot_key(account, resource, session_version) {
            return Err(identity_mismatch());
        }
        let path = self.snapshot_path(account, resource, session_version)?;
        if path.exists() || std::fs::symlink_metadata(&path).is_ok() {
            fs::validate_contained(&self.root, &path)?;
            fs::validate_owned_file(&path, true).map_err(|_| identity_mismatch())?;
        }
        Ok(path)
    }

    /// Create a unique unpublished snapshot path beside its final location.
    pub fn snapshot_staging_path(
        &self,
        account: AccountId,
        resource: ResourceId,
        session_version: u64,
    ) -> Result<PathBuf, PlatformError> {
        let history = self.ensure_history_dir(account, resource)?;
        Ok(history.join(format!(
            ".{session_version}.{}.sqlite",
            uuid::Uuid::now_v7().hyphenated()
        )))
    }

    /// Atomically publish one verified snapshot and fsync its directory entry.
    pub fn publish_snapshot(
        &self,
        staging: &Path,
        account: AccountId,
        resource: ResourceId,
        session_version: u64,
    ) -> Result<PathBuf, PlatformError> {
        let history = self.ensure_history_dir(account, resource)?;
        if staging.parent() != Some(history.as_path()) {
            return Err(identity_mismatch());
        }
        fs::validate_owned_file(staging, true).map_err(|_| identity_mismatch())?;
        let expected_prefix = format!(".{session_version}.");
        let valid_name = staging
            .file_name()
            .and_then(std::ffi::OsStr::to_str)
            .and_then(|name| name.strip_prefix(&expected_prefix))
            .and_then(|tail| tail.strip_suffix(".sqlite"))
            .is_some_and(|token| {
                uuid::Uuid::parse_str(token).is_ok_and(|id| id.hyphenated().to_string() == token)
            });
        if !valid_name {
            return Err(identity_mismatch());
        }
        let destination = history.join(format!("{session_version}.sqlite"));
        if destination.exists() || std::fs::symlink_metadata(&destination).is_ok() {
            return Err(identity_mismatch());
        }
        std::fs::rename(staging, &destination)
            .map_err(|_| path_error("failed to publish D1 snapshot"))?;
        fs::fsync_dir(&history)?;
        Ok(destination)
    }

    /// Create and validate an account directory.
    pub fn ensure_account_dir(&self, account: AccountId) -> Result<PathBuf, PlatformError> {
        let path = self.root.join(account.to_string());
        fs::create_dir_secure(&path)?;
        fs::validate_contained(&self.root, &path)?;
        Ok(path)
    }

    fn snapshot_path(
        &self,
        account: AccountId,
        resource: ResourceId,
        session_version: u64,
    ) -> Result<PathBuf, PlatformError> {
        Ok(self
            .ensure_history_dir(account, resource)?
            .join(format!("{session_version}.sqlite")))
    }

    fn ensure_history_dir(
        &self,
        account: AccountId,
        resource: ResourceId,
    ) -> Result<PathBuf, PlatformError> {
        let database = self.database_dir(account, resource);
        fs::validate_owned_dir(&database).map_err(|_| identity_mismatch())?;
        let history = database.join(HISTORY_DIR);
        fs::create_dir_secure(&history)?;
        fs::validate_contained(&self.root, &history)?;
        Ok(history)
    }

    /// Create a unique create/restore staging directory.
    pub fn create_database_staging(&self, resource: ResourceId) -> Result<PathBuf, PlatformError> {
        let name = format!("{resource}.{}", uuid::Uuid::now_v7().hyphenated());
        let path = self.root.join(STAGING_DIR).join(name);
        std::fs::create_dir(&path)
            .map_err(|_| path_error("failed to create D1 staging directory"))?;
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
            return Err(identity_mismatch());
        }
        fs::validate_owned_dir(staging)?;
        let account_dir = self.ensure_account_dir(account)?;
        let live = self.database_dir(account, resource);
        if live.exists() || std::fs::symlink_metadata(&live).is_ok() {
            return Err(identity_mismatch());
        }
        std::fs::rename(staging, &live).map_err(|_| path_error("failed to publish D1 database"))?;
        fs::fsync_dir(&account_dir)
    }

    /// Move one live database into recoverable quarantine.
    pub fn quarantine(
        &self,
        account: AccountId,
        resource: ResourceId,
    ) -> Result<Option<PathBuf>, PlatformError> {
        let live = self.database_dir(account, resource);
        if !live.exists() {
            return Ok(None);
        }
        fs::validate_owned_dir(&live)?;
        let name = format!("{resource}.{}", uuid::Uuid::now_v7().hyphenated());
        let trash = self.root.join(TRASH_DIR).join(name);
        std::fs::rename(&live, &trash)
            .map_err(|_| path_error("failed to quarantine D1 database"))?;
        fs::fsync_dir(&self.root.join(TRASH_DIR))?;
        Ok(Some(trash))
    }

    /// List canonical operation directories for an exact resource identity.
    pub fn staging_candidates(&self, resource: ResourceId) -> Result<Vec<PathBuf>, PlatformError> {
        self.operation_candidates(STAGING_DIR, resource)
    }

    /// List canonical quarantine directories for an exact resource identity.
    pub fn quarantine_candidates(
        &self,
        resource: ResourceId,
    ) -> Result<Vec<PathBuf>, PlatformError> {
        self.operation_candidates(TRASH_DIR, resource)
    }

    /// Remove a validated staging or quarantine containing only SQLite-owned files.
    pub fn remove_operation_dir(&self, path: &Path) -> Result<(), PlatformError> {
        let parent = path.parent().ok_or_else(identity_mismatch)?;
        if parent != self.root.join(STAGING_DIR) && parent != self.root.join(TRASH_DIR) {
            return Err(identity_mismatch());
        }
        fs::validate_owned_dir(path)?;
        for entry in std::fs::read_dir(path).map_err(|_| identity_mismatch())? {
            let entry = entry.map_err(|_| identity_mismatch())?;
            let kind = entry.file_type().map_err(|_| identity_mismatch())?;
            if entry.file_name() == HISTORY_DIR && kind.is_dir() && !kind.is_symlink() {
                self.remove_history_dir(&entry.path())?;
                continue;
            }
            let allowed = entry.file_name().to_str().is_some_and(|name| {
                matches!(name, DATABASE_FILE | "data.sqlite-wal" | "data.sqlite-shm")
            });
            if !allowed || kind.is_symlink() || !kind.is_file() {
                return Err(identity_mismatch());
            }
            std::fs::remove_file(entry.path()).map_err(|_| identity_mismatch())?;
        }
        std::fs::remove_dir(path).map_err(|_| identity_mismatch())?;
        fs::fsync_dir(parent)
    }

    fn remove_history_dir(&self, path: &Path) -> Result<(), PlatformError> {
        fs::validate_owned_dir(path)?;
        for entry in std::fs::read_dir(path).map_err(|_| identity_mismatch())? {
            let entry = entry.map_err(|_| identity_mismatch())?;
            let kind = entry.file_type().map_err(|_| identity_mismatch())?;
            let valid_name = entry.file_name().to_str().is_some_and(snapshot_file_name);
            if !valid_name || kind.is_symlink() || !kind.is_file() {
                return Err(identity_mismatch());
            }
            std::fs::remove_file(entry.path()).map_err(|_| identity_mismatch())?;
        }
        std::fs::remove_dir(path).map_err(|_| identity_mismatch())?;
        Ok(())
    }

    fn operation_candidates(
        &self,
        directory: &str,
        resource: ResourceId,
    ) -> Result<Vec<PathBuf>, PlatformError> {
        let parent = self.root.join(directory);
        let prefix = format!("{resource}.");
        let mut candidates = Vec::new();
        for entry in std::fs::read_dir(&parent).map_err(|_| identity_mismatch())? {
            let entry = entry.map_err(|_| identity_mismatch())?;
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
            let kind = entry.file_type().map_err(|_| identity_mismatch())?;
            if kind.is_symlink() || !kind.is_dir() {
                return Err(identity_mismatch());
            }
            candidates.push(entry.path());
        }
        candidates.sort();
        Ok(candidates)
    }
}

fn identity_mismatch() -> PlatformError {
    PlatformError::new(
        ErrorCode::D1IdentityMismatch,
        "D1 physical identity invariant failed",
    )
}

fn path_error(message: &'static str) -> PlatformError {
    PlatformError::new(ErrorCode::PathInvalid, message)
}

fn snapshot_file_name(name: &str) -> bool {
    let Some(stem) = name.strip_suffix(".sqlite") else {
        return false;
    };
    if stem.parse::<u64>().is_ok() {
        return true;
    }
    let Some(stem) = stem.strip_prefix('.') else {
        return false;
    };
    let Some((version, token)) = stem.split_once('.') else {
        return false;
    };
    version.parse::<u64>().is_ok()
        && uuid::Uuid::parse_str(token).is_ok_and(|id| id.hyphenated().to_string() == token)
}

#[cfg(test)]
#[path = "paths_tests.rs"]
mod tests;
