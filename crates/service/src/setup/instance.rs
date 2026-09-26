//! Daemon-owned creation of an instance's explicit config and fresh authority.

use super::*;
use open_compute_core::InstanceName;
use std::os::unix::fs::MetadataExt;

pub(crate) fn create_instance(
    ocd_root: &Path,
    scope: ServiceScope,
    config_path: &Path,
    data_dir: &Path,
    name: Option<&InstanceName>,
) -> Result<(), PlatformError> {
    if !config_path.is_absolute() || !data_dir.is_absolute() {
        return Err(PlatformError::new(
            ErrorCode::ConfigPathInvalid,
            "instance setup requires absolute config and data paths",
        ));
    }
    let config_path = crate::instance_registry::normalize_real_path(config_path)?;
    let data_dir = crate::instance_registry::validate_instance_data_path(ocd_root, data_dir)?;
    let config_path = config_path.as_path();
    let data_dir = data_dir.as_path();
    refuse_existing(config_path, "configuration file")?;
    refuse_nonempty_data(data_dir)?;

    let parent = config_path.parent().ok_or_else(|| {
        PlatformError::new(
            ErrorCode::ConfigPathInvalid,
            "instance config has no parent",
        )
    })?;
    let keys = data_dir.join("keys");
    let objects = data_dir.join("objects");
    let deployer = keys.join("deployer.token");
    let read_only = keys.join("read-only.token");
    let master_key = keys.join("master.key");
    ensure_config_parent(parent, scope)?;
    ensure_dir_tree(data_dir, scope)?;
    ensure_dir_tree(&keys, scope)?;
    ensure_dir_tree(&objects, scope)?;
    if crate::instance_registry::validate_instance_data_path(ocd_root, data_dir)? != data_dir {
        return Err(PlatformError::new(
            ErrorCode::PathInvalid,
            "instance data path changed while creating directories",
        ));
    }

    let mut config = PlatformConfig::from_toml_str(DEFAULT_CONFIG)?;
    config.instance.name = name.cloned();
    config.dashboard.enabled = true;
    config.data.path = data_dir.to_owned();
    config.data.master_key_file = master_key;
    config.data.master_key_env = None;
    config.auth.deployer_auth = SecretReference {
        env: None,
        file: Some(deployer.clone()),
    };
    config.auth.read_only_auth = SecretReference {
        env: None,
        file: Some(read_only.clone()),
    };
    let ObjectStorageConfig::Local(local) = &mut config.object_storage else {
        return Err(PlatformError::new(
            ErrorCode::ConfigInvalid,
            "embedded default config must use local object storage",
        ));
    };
    local.path = objects;
    config.validate()?;
    let body = toml::to_string_pretty(&config).map_err(|_| {
        PlatformError::new(
            ErrorCode::ConfigInvalid,
            "failed to serialize instance config",
        )
    })?;

    let loaded =
        publish_instance_files(config_path, data_dir, &deployer, &read_only, &body, scope)?;

    // Bootstrap may have persisted authority even if it returns an error. Keep
    // config and data for operator recovery instead of deleting uncertain state.
    drop(PlatformStorage::bootstrap_with_hardening(
        &loaded.config.data,
        &loaded.config.hardening,
        &SystemClock,
    )?);
    Ok(())
}

fn publish_instance_files(
    config_path: &Path,
    data_dir: &Path,
    deployer: &Path,
    read_only: &Path,
    body: &str,
    scope: ServiceScope,
) -> Result<crate::config_load::LoadedConfig, PlatformError> {
    let mut published = Vec::new();
    let result = (|| {
        let deployer_token = random_token()?;
        let mut read_only_token = random_token()?;
        while read_only_token == deployer_token {
            read_only_token = random_token()?;
        }
        published.push(exclusive_write_secret(deployer, &deployer_token, scope)?);
        published.push(exclusive_write_secret(read_only, &read_only_token, scope)?);
        published.push(exclusive_write_bytes(
            config_path,
            body.as_bytes(),
            0o600,
            scope,
        )?);
        let loaded = load_platform_config_from(config_path, Path::new("/"))?;
        if loaded.config.data.path != data_dir {
            return Err(PlatformError::new(
                ErrorCode::ConfigPathInvalid,
                "generated instance data path changed during validation",
            ));
        }
        Ok(loaded)
    })();
    if result.is_err() {
        remove_published_files(&published)?;
    }
    result
}

fn ensure_config_parent(path: &Path, scope: ServiceScope) -> Result<(), PlatformError> {
    let path = crate::instance_registry::normalize_real_path(path)?;
    let mut current = PathBuf::new();
    for component in path.components() {
        current.push(component.as_os_str());
        if matches!(component, std::path::Component::RootDir) {
            continue;
        }
        match fs::symlink_metadata(&current) {
            Ok(meta) if meta.is_dir() && !meta.file_type().is_symlink() => {}
            Ok(_) => {
                return Err(PlatformError::new(
                    ErrorCode::PathInvalid,
                    "instance config parent contains a symlink or non-directory",
                ));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                create_dir_mapped(&current, scope)?;
            }
            Err(_) => {
                return Err(PlatformError::new(
                    ErrorCode::PathInvalid,
                    "cannot inspect instance config parent",
                ));
            }
        }
    }
    let meta = fs::symlink_metadata(path).map_err(|_| {
        PlatformError::new(
            ErrorCode::PathInvalid,
            "cannot inspect instance config directory",
        )
    })?;
    if meta.uid() != rustix::process::getuid().as_raw() || meta.permissions().mode() & 0o022 != 0 {
        return Err(PlatformError::new(
            ErrorCode::PathInvalid,
            "instance config directory must be owned by the daemon UID and not writable by others",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{PermissionsExt, symlink};
    use tempfile::TempDir;

    #[test]
    fn creates_explicit_data_authority_and_never_scans_siblings() {
        let temp = TempDir::new().unwrap();
        let root = temp.path().join("ocd");
        let config = root.join("instances/dev/compute.toml");
        let data = temp.path().join("external-data");
        let name: InstanceName = "dev".parse().unwrap();
        create_instance(&root, ServiceScope::User, &config, &data, Some(&name)).unwrap();
        let loaded = load_platform_config_from(&config, Path::new("/")).unwrap();
        assert_eq!(loaded.config.data.path, data.canonicalize().unwrap());
        assert_eq!(loaded.config.instance.name.as_ref(), Some(&name));
        assert!(loaded.config.dashboard.enabled);
        assert!(data.join("control.sqlite").exists());
        assert!(!config.parent().unwrap().join("data").exists());
        assert_eq!(
            fs::metadata(&config).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(data.join("keys/deployer.token"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600,
        );
        let registry = InstanceRegistry::with_roots(temp.path().join("system"), root);
        assert!(registry.list_scope(ServiceScope::User).unwrap().is_empty());
    }

    #[test]
    fn creation_uses_the_verified_real_data_path() {
        let temp = TempDir::new().unwrap();
        let root = temp.path().join("ocd");
        let config = root.join("instances/dev/compute.toml");
        let actual = temp.path().join("actual");
        let alias = temp.path().join("alias");
        fs::create_dir(&actual).unwrap();
        symlink(&actual, &alias).unwrap();
        create_instance(
            &root,
            ServiceScope::User,
            &config,
            &alias.join("data"),
            None,
        )
        .unwrap();
        let loaded = load_platform_config_from(&config, Path::new("/")).unwrap();
        assert_eq!(
            loaded.config.data.path,
            actual.canonicalize().unwrap().join("data")
        );
        assert!(loaded.config.data.path.join("control.sqlite").exists());
    }

    #[test]
    fn rejects_existing_config_and_unknown_data_without_overwriting() {
        let temp = TempDir::new().unwrap();
        let root = temp.path().join("ocd");
        let config = root.join("instances/dev/compute.toml");
        let data = root.join("instances/dev/data");
        fs::create_dir_all(&data).unwrap();
        fs::write(data.join("unknown"), b"keep").unwrap();
        assert_eq!(
            create_instance(&root, ServiceScope::User, &config, &data, None)
                .unwrap_err()
                .code(),
            ErrorCode::PathInvalid,
        );
        assert_eq!(fs::read(data.join("unknown")).unwrap(), b"keep");
        assert!(!config.exists());
        fs::remove_file(data.join("unknown")).unwrap();
        fs::write(&config, b"existing").unwrap();
        assert_eq!(
            create_instance(&root, ServiceScope::User, &config, &data, None)
                .unwrap_err()
                .code(),
            ErrorCode::PathInvalid,
        );
        assert_eq!(fs::read(config).unwrap(), b"existing");
    }

    #[test]
    fn publish_failure_removes_only_files_created_by_this_attempt() {
        let temp = TempDir::new().unwrap();
        let deployer = temp.path().join("deployer.token");
        let read_only = temp.path().join("read-only.token");
        let config = temp.path().join("compute.toml");
        fs::write(&read_only, b"preexisting").unwrap();
        let error = publish_instance_files(
            &config,
            temp.path(),
            &deployer,
            &read_only,
            "unused",
            ServiceScope::User,
        )
        .unwrap_err();
        assert_eq!(error.code(), ErrorCode::PathInvalid);
        assert!(!deployer.exists() && !config.exists());
        assert_eq!(fs::read(read_only).unwrap(), b"preexisting");
    }
}
