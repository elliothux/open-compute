//! Secret-safe S3 credential resolution.

use open_compute_core::{ErrorCode, PlatformError, S3Config, SecretString};
use rustix::fs::{Mode, OFlags, open};
use rustix::io::read;
use std::fmt::{Debug, Display, Formatter};
use std::fs::File;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

/// Environment lookup used while resolving credentials.
pub trait CredentialEnv: Send + Sync {
    /// Return the value of `name` if present.
    fn get(&self, name: &str) -> Option<String>;
}

/// Process environment.
#[derive(Debug, Clone, Copy, Default)]
pub struct ProcessEnv;

impl CredentialEnv for ProcessEnv {
    fn get(&self, name: &str) -> Option<String> {
        std::env::var(name).ok()
    }
}

/// Test-only overlay that never reads the process environment.
#[derive(Clone, Debug, Default)]
pub struct MapEnv {
    values: std::collections::HashMap<String, String>,
}

impl MapEnv {
    /// Create an empty overlay.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert a name/value pair.
    #[must_use]
    pub fn with(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.values.insert(name.into(), value.into());
        self
    }
}

impl CredentialEnv for MapEnv {
    fn get(&self, name: &str) -> Option<String> {
        self.values.get(name).cloned()
    }
}

/// Static pair used by injected test resolvers.
#[derive(Clone, Debug, Default)]
pub struct StaticEnv {
    inner: MapEnv,
}

impl StaticEnv {
    /// Wrap a [`MapEnv`].
    #[must_use]
    pub const fn new(inner: MapEnv) -> Self {
        Self { inner }
    }
}

impl CredentialEnv for StaticEnv {
    fn get(&self, name: &str) -> Option<String> {
        self.inner.get(name)
    }
}

/// Resolved S3 credentials. Debug/Display never include secret material.
pub struct S3Credentials {
    access_key_id: SecretString,
    secret_access_key: SecretString,
}

impl S3Credentials {
    /// Access key.
    #[must_use]
    pub fn access_key_id(&self) -> &SecretString {
        &self.access_key_id
    }

    /// Secret key.
    #[must_use]
    pub fn secret_access_key(&self) -> &SecretString {
        &self.secret_access_key
    }
}

impl Debug for S3Credentials {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("S3Credentials")
            .field("access_key_id", &self.access_key_id)
            .field("secret_access_key", &self.secret_access_key)
            .finish()
    }
}

impl Display for S3Credentials {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str("S3Credentials([REDACTED])")
    }
}

/// Resolve credentials from the process environment and configured files.
pub fn resolve_s3_credentials(config: &S3Config) -> Result<S3Credentials, PlatformError> {
    resolve_s3_credentials_with(config, &ProcessEnv)
}

/// Resolve credentials using an injected environment lookup.
pub fn resolve_s3_credentials_with(
    config: &S3Config,
    env: &dyn CredentialEnv,
) -> Result<S3Credentials, PlatformError> {
    let access = resolve_pair(
        config.access_key_id_env.as_deref(),
        config.access_key_id_file.as_deref(),
        env,
    )?;
    let secret = resolve_pair(
        config.secret_access_key_env.as_deref(),
        config.secret_access_key_file.as_deref(),
        env,
    )?;
    Ok(S3Credentials {
        access_key_id: access,
        secret_access_key: secret,
    })
}

fn resolve_pair(
    env_name: Option<&str>,
    file: Option<&Path>,
    env: &dyn CredentialEnv,
) -> Result<SecretString, PlatformError> {
    let from_env = match env_name {
        Some(name) => match env.get(name) {
            Some(value) if !value.is_empty() => Some(value),
            Some(_) => {
                return Err(PlatformError::new(
                    ErrorCode::SecretRefInvalid,
                    "s3 credential environment value is empty",
                ));
            }
            None if file.is_none() => {
                return Err(PlatformError::new(
                    ErrorCode::SecretRefInvalid,
                    "s3 credential environment variable is missing",
                ));
            }
            None => None,
        },
        None => None,
    };
    let from_file = match file {
        Some(path) => Some(read_credential_file(path)?),
        None => None,
    };
    match (from_env, from_file) {
        (Some(a), Some(b)) if a == b => Ok(SecretString::new(a)),
        (Some(_), Some(_)) => Err(PlatformError::new(
            ErrorCode::SecretRefInvalid,
            "s3 credential env and file values do not match",
        )),
        (Some(a), None) | (None, Some(a)) => Ok(SecretString::new(a)),
        (None, None) => Err(PlatformError::new(
            ErrorCode::SecretRefInvalid,
            "s3 credential reference is missing",
        )),
    }
}

fn read_credential_file(path: &Path) -> Result<String, PlatformError> {
    if !path.is_absolute() {
        return Err(PlatformError::new(
            ErrorCode::PathInvalid,
            "s3 credential file must be an absolute path",
        ));
    }
    let fd = open(path, OFlags::RDONLY | OFlags::NOFOLLOW, Mode::empty()).map_err(|_| {
        PlatformError::new(
            ErrorCode::SecretRefInvalid,
            "s3 credential file could not be opened without following links",
        )
    })?;
    let file = File::from(fd);
    let meta = file.metadata().map_err(|_| {
        PlatformError::new(
            ErrorCode::SecretRefInvalid,
            "s3 credential file could not be inspected",
        )
    })?;
    if !meta.file_type().is_file() {
        return Err(PlatformError::new(
            ErrorCode::SecretRefInvalid,
            "s3 credential file must be a regular file",
        ));
    }
    if meta.permissions().mode() & 0o777 != 0o600 {
        return Err(PlatformError::new(
            ErrorCode::SecretRefInvalid,
            "s3 credential file must have mode 0600",
        ));
    }
    let mut bytes = Vec::new();
    let mut chunk = [0_u8; 4096];
    loop {
        let n = read(&file, &mut chunk).map_err(|_| {
            PlatformError::new(
                ErrorCode::SecretRefInvalid,
                "s3 credential file could not be read",
            )
        })?;
        if n == 0 {
            break;
        }
        bytes.extend_from_slice(&chunk[..n]);
        if bytes.len() > 16 * 1024 {
            return Err(PlatformError::new(
                ErrorCode::SecretRefInvalid,
                "s3 credential file is corrupt",
            ));
        }
    }
    let buf = String::from_utf8(bytes).map_err(|_| {
        PlatformError::new(ErrorCode::SecretRefInvalid, "s3 credential file is corrupt")
    })?;
    let value = buf.trim_end_matches(['\n', '\r']).to_string();
    if value.is_empty() {
        return Err(PlatformError::new(
            ErrorCode::SecretRefInvalid,
            "s3 credential file is empty",
        ));
    }
    if value.as_bytes().contains(&0) {
        return Err(PlatformError::new(
            ErrorCode::SecretRefInvalid,
            "s3 credential file is corrupt",
        ));
    }
    Ok(value)
}
