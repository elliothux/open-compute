use crate::{ErrorCode, PlatformError};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// One operator-configured local native extension directory.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct LocalExtensionConfig {
    /// Absolute extension directory after config-relative path resolution.
    pub path: PathBuf,
}

impl LocalExtensionConfig {
    pub(super) fn validate(&self) -> Result<(), PlatformError> {
        super::require_absolute(&self.path, "extensions.<name>.path")
    }
}

/// Validate the shared lowercase service-name syntax used by Workers and local extensions.
pub fn validate_local_extension_name(name: &str) -> Result<(), PlatformError> {
    let bytes = name.as_bytes();
    if bytes.is_empty()
        || bytes.len() > 63
        || !bytes[0].is_ascii_alphanumeric()
        || !bytes[bytes.len() - 1].is_ascii_alphanumeric()
        || bytes
            .iter()
            .any(|byte| !byte.is_ascii_lowercase() && !byte.is_ascii_digit() && *byte != b'-')
    {
        return Err(PlatformError::new(
            ErrorCode::ConfigInvalid,
            "local extension name must be a lowercase ASCII slug",
        ));
    }
    Ok(())
}
