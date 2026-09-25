use super::*;

pub(super) struct UpgradeBackups {
    pub(super) binary: PathBuf,
    pub(super) receipt: PathBuf,
}

impl UpgradeBackups {
    pub(super) fn new(options: &UpgradeOptions) -> Result<Self, PlatformError> {
        Ok(Self {
            binary: backup_path(&options.binary_path)?,
            receipt: backup_path(&options.receipt_path)?,
        })
    }

    pub(super) fn ensure_absent(&self) -> Result<(), PlatformError> {
        if self.binary.exists() || self.receipt.exists() {
            return Err(PlatformError::new(
                ErrorCode::ReleaseUnsupported,
                "an unfinished upgrade backup exists; run `ocd upgrade --restore`",
            ));
        }
        Ok(())
    }

    pub(super) fn create(options: &UpgradeOptions) -> Result<Self, PlatformError> {
        let backups = Self::new(options)?;
        backups.ensure_absent()?;
        fs::hard_link(&options.binary_path, &backups.binary).map_err(|_| {
            PlatformError::new(
                ErrorCode::PathInvalid,
                "failed to preserve the installed binary",
            )
        })?;
        let Ok(receipt_bytes) = fs::read(&options.receipt_path) else {
            let _ = fs::remove_file(&backups.binary);
            return Err(PlatformError::new(
                ErrorCode::PathInvalid,
                "failed to preserve the install receipt",
            ));
        };
        let result = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&backups.receipt)
            .and_then(|mut file| {
                file.write_all(&receipt_bytes)?;
                file.sync_all()
            });
        if result.is_err() {
            let _ = fs::remove_file(&backups.binary);
            return Err(PlatformError::new(
                ErrorCode::PathInvalid,
                "failed to preserve the install receipt",
            ));
        }
        Ok(backups)
    }

    pub(super) fn restore(&self, options: &UpgradeOptions) -> Result<(), PlatformError> {
        let staged = options
            .staging_dir
            .join(format!(".ocd-restore-{}", Uuid::now_v7().as_hyphenated()));
        fs::copy(&self.binary, &staged).map_err(|_| {
            PlatformError::new(
                ErrorCode::PathInvalid,
                "failed to stage the previous binary",
            )
        })?;
        fs::set_permissions(&staged, fs::Permissions::from_mode(0o755)).map_err(|_| {
            PlatformError::new(
                ErrorCode::PathInvalid,
                "failed to protect the restored binary",
            )
        })?;
        atomic_replace_binary(&staged, &options.binary_path)?;
        let receipt = install_receipt::read_receipt(&self.receipt)?;
        write_receipt(&options.receipt_path, &receipt)?;
        Ok(())
    }

    pub(super) fn remove(&self) -> Result<(), PlatformError> {
        fs::remove_file(&self.binary).map_err(|_| {
            PlatformError::new(
                ErrorCode::PathInvalid,
                "failed to remove upgrade binary backup",
            )
        })?;
        fs::remove_file(&self.receipt).map_err(|_| {
            PlatformError::new(
                ErrorCode::PathInvalid,
                "failed to remove upgrade receipt backup",
            )
        })?;
        sync_parent(&self.binary);
        sync_parent(&self.receipt);
        Ok(())
    }
}

/// Restore the retained pre-upgrade binary and receipt after an interrupted upgrade.
pub fn run_upgrade_restore(
    options: &UpgradeOptions,
    registry: &InstanceRegistry,
    manager: &dyn ServiceManager,
    out: &mut impl Write,
) -> Result<(), PlatformError> {
    let backups = UpgradeBackups::new(options)?;
    if !backups.binary.is_file() || !backups.receipt.is_file() {
        return Err(PlatformError::new(
            ErrorCode::ReleaseUnsupported,
            "a complete upgrade backup is not available",
        ));
    }
    let receipt = install_receipt::read_receipt(&backups.receipt)?;
    let bytes = fs::read(&backups.binary).map_err(|_| {
        PlatformError::new(
            ErrorCode::PathInvalid,
            "failed to read the upgrade binary backup",
        )
    })?;
    if hex::encode(Sha256::digest(&bytes)) != receipt.sha256
        || receipt.binary_path != options.binary_path.to_string_lossy()
    {
        return Err(PlatformError::new(
            ErrorCode::ArtifactIntegrityError,
            "upgrade backup binary and receipt do not match",
        ));
    }
    backups.restore(options)?;
    if manager.is_active(options.scope)? {
        manager.restart(options.scope)?;
        wait_scoped_daemon_state(registry, manager, options.scope, true)?;
    }
    backups.remove()?;
    writeln!(out, "UPGRADE_RESTORE_OK {}", receipt.version).map_err(|_| io_failed())?;
    Ok(())
}

pub(super) fn backup_path(path: &Path) -> Result<PathBuf, PlatformError> {
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| {
            PlatformError::new(ErrorCode::PathInvalid, "upgrade path has no UTF-8 filename")
        })?;
    Ok(path.with_file_name(format!(".{name}.upgrade-backup")))
}
