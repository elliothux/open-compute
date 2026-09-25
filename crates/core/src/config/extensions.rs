use crate::{ErrorCode, InstanceId, PlatformError, SecretReference, VersionId, WorkerId};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::PathBuf;

/// One operator-configured local native extension directory.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct LocalExtensionConfig {
    /// Absolute extension directory after config-relative path resolution.
    pub path: PathBuf,
}

/// One immutable caller grant for an operator-owned private HTTP Service target.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PrivateHttpGrant {
    /// Account/instance allowed to bind the service.
    pub account_id: InstanceId,
    /// Exact caller Worker identity.
    pub worker_id: WorkerId,
    /// Exact immutable caller Version identity.
    #[serde(default)]
    pub version_id: Option<VersionId>,
    /// Optional Service Binding entrypoint; omitted means default fetch only.
    #[serde(default)]
    pub entrypoint: Option<String>,
}

/// Operator-owned fixed private HTTP destination exposed through a normal Service Binding.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PrivateHttpServiceConfig {
    /// `http` or `https`.
    pub scheme: String,
    /// Fixed IP literal or DNS hostname pinned when `ocd` starts.
    pub host: String,
    /// Fixed destination port.
    pub port: u16,
    /// Allowed absolute path prefixes.
    pub path_prefixes: Vec<String>,
    /// Allowed uppercase HTTP methods.
    pub methods: BTreeSet<String>,
    /// Optional credential header injected only by `ocd`.
    #[serde(default)]
    pub credential_header: Option<String>,
    /// Optional credential value loaded from an env/file reference.
    #[serde(default)]
    pub credential: Option<SecretReference>,
    /// Exact immutable callers allowed to bind this target.
    pub allow: Vec<PrivateHttpGrant>,
}

impl PrivateHttpServiceConfig {
    pub(super) fn validate(&self) -> Result<(), PlatformError> {
        if !matches!(self.scheme.as_str(), "http" | "https")
            || self.host.is_empty()
            || self.host.len() > 253
            || self.host.bytes().any(|byte| byte.is_ascii_control())
            || self.port == 0
            || self.path_prefixes.is_empty()
            || self.methods.is_empty()
            || self.allow.is_empty()
        {
            return Err(invalid_private_http());
        }
        if self.path_prefixes.iter().any(|path| {
            !path.starts_with('/') || path.contains("..") || path.contains(['?', '#', '\\'])
        }) || self.methods.iter().any(|method| {
            method.is_empty()
                || method.len() > 16
                || method.bytes().any(|byte| !byte.is_ascii_uppercase())
        }) {
            return Err(invalid_private_http());
        }
        match (&self.credential_header, &self.credential) {
            (None, None) => {}
            (Some(header), Some(secret)) => {
                if header.is_empty()
                    || header.len() > 128
                    || header
                        .bytes()
                        .any(|byte| !byte.is_ascii_alphanumeric() && !matches!(byte, b'-' | b'_'))
                {
                    return Err(invalid_private_http());
                }
                secret.validate("private_services.<name>.credential")?;
            }
            _ => return Err(invalid_private_http()),
        }
        if self.allow.iter().any(|grant| {
            grant.entrypoint.as_deref().is_some_and(|entrypoint| {
                entrypoint.is_empty()
                    || entrypoint.len() > 128
                    || entrypoint
                        .bytes()
                        .any(|byte| !byte.is_ascii_alphanumeric() && !matches!(byte, b'_' | b'$'))
            })
        }) {
            return Err(invalid_private_http());
        }
        Ok(())
    }
}

fn invalid_private_http() -> PlatformError {
    PlatformError::new(
        ErrorCode::ConfigInvalid,
        "private HTTP Service target configuration is invalid",
    )
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
