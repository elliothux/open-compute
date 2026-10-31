//! Manifest mutations owned by the running scoped daemon.

use super::*;

impl InstanceRegistry {
    pub(crate) fn add_online(
        &self,
        scope: ServiceScope,
        config_path: &Path,
        autostart: bool,
        expected: &[InstanceRecord],
    ) -> Result<(InstanceRecord, crate::run::daemon_control::RegisteredTokens), PlatformError> {
        let loaded = crate::config_load::load_platform_config_from(config_path, Path::new("/"))?;
        if expected
            .iter()
            .any(|record| record.config_path() == loaded.path)
        {
            return Err(manifest_invalid(
                "instance configuration is already registered",
            ));
        }
        let candidate = load_record(self.root_for(scope), &loaded.path, scope, autostart)?;
        let mut records = expected.to_vec();
        records.push(candidate);
        validate_unique_records(&records)?;
        let credentials =
            crate::run::validate_registered_tokens(&records, &self.server_config(scope)?)?
                .pop()
                .ok_or_else(|| manifest_invalid("new instance credential scope is missing"))?;
        let record =
            self.register_inner(&loaded.path, scope, None, autostart, SystemTime::now())?;
        Ok((record, credentials))
    }

    pub(crate) fn remove_online(&self, record: &InstanceRecord) -> Result<(), PlatformError> {
        self.remove_record(record)
    }
}
