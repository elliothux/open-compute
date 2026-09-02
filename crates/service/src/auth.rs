//! Admin Bearer authentication using constant-time comparison.

use open_compute_core::config::SecretReference;
use open_compute_core::{ErrorCode, PlatformError, SecretString};
use rustix::fs::{Mode, OFlags};
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use subtle::ConstantTimeEq;

const MAX_SECRET_BYTES: usize = 256;

/// Resolve `server.admin_auth` from env and/or a 0600 no-follow file.
pub fn resolve_admin_auth(reference: &SecretReference) -> Result<SecretString, PlatformError> {
    resolve_bearer_auth(reference)
}

/// Resolve one Bearer token using the same bounded env/file authority.
pub(crate) fn resolve_bearer_auth(
    reference: &SecretReference,
) -> Result<SecretString, PlatformError> {
    let from_env = match &reference.env {
        Some(name) => match std::env::var(name) {
            Ok(value) if !value.is_empty() && value.len() <= MAX_SECRET_BYTES => Some(value),
            Ok(value) if value.len() > MAX_SECRET_BYTES => {
                return Err(PlatformError::new(
                    ErrorCode::SecretRefInvalid,
                    "Bearer auth secret exceeds the bounded size",
                ));
            }
            Ok(_) => {
                return Err(PlatformError::new(
                    ErrorCode::SecretRefInvalid,
                    "Bearer auth environment value is empty",
                ));
            }
            Err(_) if reference.file.is_none() => {
                return Err(PlatformError::new(
                    ErrorCode::SecretRefInvalid,
                    "Bearer auth environment variable is missing",
                ));
            }
            Err(_) => None,
        },
        None => None,
    };
    let from_file = match &reference.file {
        Some(path) => Some(read_secret_file(path)?),
        None => None,
    };
    match (from_env, from_file) {
        (Some(a), Some(b)) if a == b => Ok(SecretString::new(a)),
        (Some(_), Some(_)) => Err(PlatformError::new(
            ErrorCode::SecretRefInvalid,
            "Bearer auth env and file values do not match",
        )),
        (Some(a), None) | (None, Some(a)) => Ok(SecretString::new(a)),
        (None, None) => Err(PlatformError::new(
            ErrorCode::SecretRefInvalid,
            "Bearer auth reference is missing",
        )),
    }
}

fn read_secret_file(path: &Path) -> Result<String, PlatformError> {
    if !path.is_absolute()
        || path
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err(PlatformError::new(
            ErrorCode::PathInvalid,
            "configured Bearer auth path must be absolute",
        ));
    }
    let fd = rustix::fs::open(
        path,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|_| {
        PlatformError::new(
            ErrorCode::SecretRefInvalid,
            "Bearer auth file could not be opened without following links",
        )
    })?;
    let file = File::from(fd);
    let meta = file.metadata().map_err(|_| {
        PlatformError::new(
            ErrorCode::SecretRefInvalid,
            "Bearer auth file could not be inspected",
        )
    })?;
    if !meta.file_type().is_file() {
        return Err(PlatformError::new(
            ErrorCode::SecretRefInvalid,
            "Bearer auth file must be a regular file",
        ));
    }
    if meta.permissions().mode() & 0o777 != 0o600 {
        return Err(PlatformError::new(
            ErrorCode::SecretRefInvalid,
            "Bearer auth file must have mode 0600",
        ));
    }
    if meta.len() == 0 || meta.len() as usize > MAX_SECRET_BYTES {
        return Err(PlatformError::new(
            ErrorCode::SecretRefInvalid,
            "Bearer auth secret exceeds the bounded size",
        ));
    }
    let mut buf = String::new();
    let mut file = file;
    file.read_to_string(&mut buf).map_err(|_| {
        PlatformError::new(
            ErrorCode::SecretRefInvalid,
            "Bearer auth file is not valid UTF-8",
        )
    })?;
    let trimmed = buf.trim_end_matches(['\n', '\r']);
    if trimmed.is_empty() {
        return Err(PlatformError::new(
            ErrorCode::SecretRefInvalid,
            "Bearer auth file is empty",
        ));
    }
    Ok(trimmed.to_string())
}

/// Constant-time comparison of `Authorization` against `Bearer <secret>`.
#[must_use]
pub fn bearer_matches(header: Option<&str>, secret: &SecretString) -> bool {
    let presented = header.unwrap_or("");
    let expected = format!("Bearer {}", secret.expose());
    let left = Sha256::digest(presented.as_bytes());
    let right = Sha256::digest(expected.as_bytes());
    bool::from(left.as_slice().ct_eq(right.as_slice()))
}
