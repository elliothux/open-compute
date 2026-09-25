//! Restore target and identity checks against the explicit OCD manifest.

use super::*;

impl InstanceRegistry {
    /// Refuse a restore target that would replace or nest inside another registered instance.
    /// Return identities of other present authorities for snapshot identity validation.
    /// The target's control database may be absent during cold recovery.
    pub(crate) fn validate_restore_target(
        &self,
        scope: ServiceScope,
        config_path: &Path,
        data_path: &Path,
    ) -> Result<Vec<InstanceId>, PlatformError> {
        let root = self.root_for(scope);
        let target = validate_instance_data_path(root, data_path)?;
        let mut matched = false;
        let mut other_ids = Vec::new();
        for entry in read_manifest(root)?.instances {
            let registered_config = resolve_registered_config(root, &entry.config)?;
            let loaded =
                crate::config_load::load_platform_config_from(&registered_config, Path::new("/"))?;
            let registered_data = validate_instance_data_path(root, &loaded.config.data.path)?;
            if target == registered_data && config_path == registered_config && !matched {
                matched = true;
                continue;
            }
            if target.starts_with(&registered_data) || registered_data.starts_with(&target) {
                return Err(manifest_invalid(
                    "restore target overlaps another registered instance data directory",
                ));
            }
            let control = registered_data.join("control.sqlite");
            match fs::symlink_metadata(&control) {
                Ok(_) => {
                    let (_, identity) =
                        inspect_control_db(&control, loaded.config.data.sqlite_busy_timeout_ms)
                            .map_err(|_| {
                                manifest_invalid("another registered instance authority is invalid")
                            })?;
                    other_ids.push(identity.instance_id);
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => {
                    return Err(manifest_invalid(
                        "another registered instance authority could not be inspected",
                    ));
                }
            }
        }
        if !matched {
            return Err(PlatformError::new(
                ErrorCode::InstanceNotFound,
                "restore configuration is not registered in this OCD scope",
            ));
        }
        Ok(other_ids)
    }
}
