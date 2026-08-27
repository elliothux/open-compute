//! Fail-closed fresh-host restore staging and atomic publication.

use crate::{ControlDb, inspect_control_db, inspect_scheduler_db};
use open_compute_core::{
    AccountId, ErrorCode, PlatformError, PlatformSnapshotManifestV1, ResourceId, SnapshotFileRole,
};
use rusqlite::{Connection, OpenFlags};
use rustix::fs::{FlockOperation, Mode, OFlags, flock};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::Read as _;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Component, Path, PathBuf};
use std::str::FromStr as _;
use uuid::Uuid;

/// Exclusive owner of a fresh-host sibling staging directory.
#[derive(Debug)]
pub struct RestoreTarget {
    target: PathBuf,
    parent: PathBuf,
    staging: PathBuf,
    staging_id: String,
    _lock: File,
    published: bool,
}

impl RestoreTarget {
    /// Validate a nonexistent or empty target, lock its canonical parent, and create staging.
    pub fn acquire(target: &Path) -> Result<Self, PlatformError> {
        crate::fs::require_absolute(target)?;
        if target
            .components()
            .any(|component| matches!(component, Component::ParentDir))
        {
            return Err(restore_invalid());
        }
        let parent = target.parent().ok_or_else(restore_invalid)?;
        let canonical_parent = std::fs::canonicalize(parent).map_err(|_| restore_invalid())?;
        if canonical_parent != parent {
            return Err(restore_invalid());
        }
        crate::fs::validate_owned_dir(&canonical_parent)?;
        let name = target
            .file_name()
            .and_then(|value| value.to_str())
            .filter(|value| !value.is_empty() && value.len() <= 128)
            .ok_or_else(restore_invalid)?;
        validate_empty_target(target)?;
        let lock_path = canonical_parent.join(format!(".{name}.restore.lock"));
        let lock = File::from(
            rustix::fs::open(
                &lock_path,
                OFlags::RDWR | OFlags::CREATE | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::RUSR | Mode::WUSR,
            )
            .map_err(|_| restore_invalid())?,
        );
        crate::fs::validate_authority_fd(&lock)?;
        flock(&lock, FlockOperation::NonBlockingLockExclusive).map_err(|_| {
            PlatformError::new(
                ErrorCode::DataDirInUse,
                "restore target is owned by another offline command",
            )
        })?;
        validate_empty_target(target)?;
        let staging_id = Uuid::now_v7().hyphenated().to_string();
        let staging = canonical_parent.join(format!(".{name}.restore-{staging_id}"));
        std::fs::create_dir(&staging).map_err(|_| restore_invalid())?;
        crate::fs::chmod(&staging, 0o700)?;
        crate::fs::validate_owned_dir(&staging)?;
        Ok(Self {
            target: target.to_path_buf(),
            parent: canonical_parent,
            staging,
            staging_id,
            _lock: lock,
            published: false,
        })
    }

    /// Absolute owned staging root.
    #[must_use]
    pub fn staging_root(&self) -> &Path {
        &self.staging
    }

    /// Create restrictive ancestors and return a path for one validated manifest entry.
    pub fn destination_for(&self, relative: &str) -> Result<PathBuf, PlatformError> {
        if !open_compute_core::valid_restore_path(relative) {
            return Err(restore_invalid());
        }
        let destination = self.staging.join(relative);
        let parent = destination.parent().ok_or_else(restore_invalid)?;
        let mut current = self.staging.clone();
        for component in parent
            .strip_prefix(&self.staging)
            .map_err(|_| restore_invalid())?
            .components()
        {
            let Component::Normal(value) = component else {
                return Err(restore_invalid());
            };
            current.push(value);
            if !current.exists() {
                std::fs::create_dir(&current).map_err(|_| restore_invalid())?;
                crate::fs::chmod(&current, 0o700)?;
            }
            crate::fs::validate_owned_dir(&current)?;
        }
        if destination.exists() || std::fs::symlink_metadata(&destination).is_ok() {
            return Err(restore_invalid());
        }
        Ok(destination)
    }

    /// Validate restored local authority and atomically install the complete data directory.
    pub fn validate_and_publish(
        mut self,
        manifest: &PlatformSnapshotManifestV1,
        master_key_fingerprint: &str,
        sqlite_busy_timeout_ms: u64,
        receipt: &[u8],
    ) -> Result<PathBuf, PlatformError> {
        validate_staging(
            &self.staging,
            manifest,
            master_key_fingerprint,
            sqlite_busy_timeout_ms,
        )?;
        normalize_restored_scheduler(
            &self.staging.join("scheduler.sqlite"),
            sqlite_busy_timeout_ms,
            manifest.created_at_ms,
        )?;
        crate::data_dir::initialize_restored_layout(&self.staging).map_err(|error| {
            restore_stage(
                &error,
                "restore excluded local layout could not be recreated",
            )
        })?;
        let operations = self.staging.join("operations");
        std::fs::create_dir(&operations).map_err(|_| restore_invalid())?;
        crate::fs::chmod(&operations, 0o700)?;
        crate::fs::atomic_write(&operations.join("last-restore.json"), receipt)?;
        sync_tree(&self.staging)?;
        validate_empty_target(&self.target)?;
        if self.target.exists() {
            std::fs::remove_dir(&self.target).map_err(|_| restore_invalid())?;
        }
        std::fs::rename(&self.staging, &self.target).map_err(|_| restore_invalid())?;
        crate::fs::fsync_dir(&self.parent)?;
        self.published = true;
        Ok(self.target.clone())
    }
}

impl Drop for RestoreTarget {
    fn drop(&mut self) {
        // Failure staging is intentionally retained for explicit, exact operator cleanup.
        if !self.published {
            let _ = crate::restore_cleanup::record_restore_failure(
                &self.target,
                &self.parent,
                &self.staging_id,
            );
        }
        let _ = flock(&self._lock, FlockOperation::Unlock);
    }
}

fn validate_staging(
    root: &Path,
    manifest: &PlatformSnapshotManifestV1,
    master_key_fingerprint: &str,
    busy_timeout_ms: u64,
) -> Result<(), PlatformError> {
    let expected_paths: BTreeSet<&str> = manifest
        .files
        .iter()
        .map(|file| file.restore_path.as_str())
        .collect();
    let actual_paths = enumerate_regular_files(root)
        .map_err(|error| restore_stage(&error, "restore staged file inventory is invalid"))?;
    if expected_paths != actual_paths.iter().map(String::as_str).collect() {
        return Err(PlatformError::new(
            ErrorCode::RestoreInvalid,
            "restore staged file inventory does not match the authenticated manifest",
        ));
    }
    for entry in &manifest.files {
        let path = root.join(&entry.restore_path);
        crate::fs::validate_owned_file(&path, true)
            .map_err(|error| restore_stage(&error, "restore staged file ownership is invalid"))?;
        let metadata = std::fs::metadata(&path).map_err(|_| restore_invalid())?;
        if metadata.len() != entry.size
            || metadata.permissions().mode() & 0o777 != 0o600
            || hash_file(&path)? != entry.sha256
        {
            return Err(PlatformError::new(
                ErrorCode::RestoreInvalid,
                "restore staged file bytes do not match the authenticated manifest",
            ));
        }
        if !matches!(entry.role, SnapshotFileRole::DurableObjectFile) {
            sqlite_quick_check(&path, busy_timeout_ms)
                .map_err(|error| restore_stage(&error, "restore staged SQLite file is invalid"))?;
        }
    }
    let (control_schema, identity) =
        inspect_control_db(&root.join("control.sqlite"), busy_timeout_ms)
            .map_err(|error| restore_stage(&error, "restore control authority is invalid"))?;
    if identity.platform_id.to_string() != manifest.platform_id
        || identity.master_key_id != master_key_fingerprint
        || u32::try_from(control_schema).ok() != manifest.source_schemas.get("control").copied()
    {
        return Err(PlatformError::new(
            ErrorCode::RestoreInvalid,
            "restore control identity does not match the authenticated manifest",
        ));
    }
    let scheduler = crate::scheduler::inspect_scheduler_schema_version(
        &root.join("scheduler.sqlite"),
        busy_timeout_ms,
    )
    .map_err(|error| restore_stage(&error, "restore scheduler authority is invalid"))?;
    if u32::try_from(scheduler).ok() != manifest.source_schemas.get("scheduler").copied() {
        return Err(PlatformError::new(
            ErrorCode::RestoreInvalid,
            "restore scheduler schema does not match the authenticated manifest",
        ));
    }
    validate_resource_catalog(root, busy_timeout_ms, manifest)
        .map_err(|error| restore_stage(&error, "restore resource catalog is invalid"))
}

fn validate_resource_catalog(
    root: &Path,
    busy_timeout_ms: u64,
    manifest: &PlatformSnapshotManifestV1,
) -> Result<(), PlatformError> {
    let control = ControlDb::open_readonly(&root.join("control.sqlite"), busy_timeout_ms)?;
    let expected = control.with_read(|connection| {
        let mut statement = connection
            .prepare(
                "SELECT r.account_id, r.id, r.kind, COALESCE(k.storage_key, d.storage_key)
                 FROM resources r
                 LEFT JOIN kv_namespaces k ON k.resource_id = r.id
                 LEFT JOIN d1_databases d ON d.resource_id = r.id
                 WHERE r.state != 'tombstoned' AND r.kind IN ('kv_namespace', 'd1_database')
                 ORDER BY r.kind, r.account_id, r.id",
            )
            .map_err(|_| restore_invalid())?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })
            .map_err(|_| restore_invalid())?;
        let mut expected = BTreeMap::new();
        for row in rows {
            let (account, resource, kind, storage_key) = row.map_err(|_| restore_invalid())?;
            let account = AccountId::from_str(&account).map_err(|_| restore_invalid())?;
            let resource = ResourceId::from_str(&resource).map_err(|_| restore_invalid())?;
            let product = match kind.as_str() {
                "kv_namespace" => "kv",
                "d1_database" => "d1",
                _ => return Err(restore_invalid()),
            };
            if storage_key != format!("v1/{account}/{resource}/data.sqlite") {
                return Err(restore_invalid());
            }
            expected.insert(
                resource.to_string(),
                format!("{product}/{account}/{resource}/data.sqlite"),
            );
        }
        Ok(expected)
    })?;
    let actual: BTreeMap<String, String> = manifest
        .files
        .iter()
        .filter(|entry| {
            matches!(
                entry.role,
                SnapshotFileRole::KvSqlite | SnapshotFileRole::D1Sqlite
            )
        })
        .map(|entry| (entry.logical_id.clone(), entry.restore_path.clone()))
        .collect();
    if expected != actual {
        return Err(restore_invalid());
    }
    Ok(())
}

fn validate_empty_target(target: &Path) -> Result<(), PlatformError> {
    match std::fs::symlink_metadata(target) {
        Ok(metadata) if metadata.file_type().is_dir() && !metadata.file_type().is_symlink() => {
            if std::fs::read_dir(target)
                .map_err(|_| restore_invalid())?
                .next()
                .is_some()
            {
                return Err(restore_invalid());
            }
            Ok(())
        }
        Ok(_) => Err(restore_invalid()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(restore_invalid()),
    }
}

fn enumerate_regular_files(root: &Path) -> Result<BTreeSet<String>, PlatformError> {
    let mut pending = vec![root.to_path_buf()];
    let mut files = BTreeSet::new();
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(&directory).map_err(|_| restore_invalid())? {
            let entry = entry.map_err(|_| restore_invalid())?;
            let kind = entry.file_type().map_err(|_| restore_invalid())?;
            if kind.is_symlink() || !(kind.is_dir() || kind.is_file()) {
                return Err(restore_invalid());
            }
            if kind.is_dir() {
                crate::fs::validate_owned_dir(&entry.path())?;
                pending.push(entry.path());
            } else {
                let relative = entry
                    .path()
                    .strip_prefix(root)
                    .map_err(|_| restore_invalid())?
                    .to_str()
                    .ok_or_else(restore_invalid)?
                    .to_owned();
                files.insert(relative);
            }
        }
    }
    Ok(files)
}

fn sqlite_quick_check(path: &Path, busy_timeout_ms: u64) -> Result<(), PlatformError> {
    let open_path = crate::control_db::leaf_nofollow_path(path)?;
    let connection = Connection::open_with_flags(
        open_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .map_err(|_| restore_invalid())?;
    connection
        .busy_timeout(std::time::Duration::from_millis(busy_timeout_ms))
        .map_err(|_| restore_invalid())?;
    let result: String = connection
        .pragma_query_value(None, "quick_check", |row| row.get(0))
        .map_err(|_| restore_invalid())?;
    if result != "ok" {
        return Err(restore_invalid());
    }
    Ok(())
}

fn normalize_restored_scheduler(
    path: &Path,
    busy_timeout_ms: u64,
    now_ms: i64,
) -> Result<(), PlatformError> {
    drop(
        crate::SchedulerStore::open(path, busy_timeout_ms, now_ms).map_err(|error| {
            restore_stage(
                &error,
                "restore scheduler runtime mode could not be initialized",
            )
        })?,
    );
    let inspection = inspect_scheduler_db(path, busy_timeout_ms, now_ms).map_err(|error| {
        restore_stage(
            &error,
            "restore scheduler runtime mode could not be verified",
        )
    })?;
    if !inspection.journal_mode.eq_ignore_ascii_case("wal")
        || inspection.synchronous != 2
        || inspection.invalid_rows != 0
    {
        return Err(restore_invalid());
    }
    Ok(())
}

fn hash_file(path: &Path) -> Result<String, PlatformError> {
    let mut file = File::open(path).map_err(|_| restore_invalid())?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(|_| restore_invalid())?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(hex::encode(digest.finalize()))
}

fn sync_tree(root: &Path) -> Result<(), PlatformError> {
    let mut directories = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        directories.push(directory.clone());
        for entry in std::fs::read_dir(&directory).map_err(|_| restore_invalid())? {
            let entry = entry.map_err(|_| restore_invalid())?;
            let kind = entry.file_type().map_err(|_| restore_invalid())?;
            if kind.is_dir() {
                pending.push(entry.path());
            } else if kind.is_file() {
                File::open(entry.path())
                    .and_then(|file| file.sync_all())
                    .map_err(|_| restore_invalid())?;
            } else {
                return Err(restore_invalid());
            }
        }
    }
    directories.sort_by_key(|path| std::cmp::Reverse(path.components().count()));
    for directory in directories {
        crate::fs::fsync_dir(&directory)?;
    }
    Ok(())
}

fn restore_invalid() -> PlatformError {
    PlatformError::new(
        ErrorCode::RestoreInvalid,
        "fresh-host restore failed validation",
    )
}

fn restore_stage(error: &PlatformError, message: &'static str) -> PlatformError {
    PlatformError::new(error.code(), message)
}
