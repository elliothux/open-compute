//! Master-key resolution, generation, and fingerprinting.

use crate::fs;
use open_compute_core::config::DataConfig;
use open_compute_core::{ErrorCode, PlatformError, SecretBytes};
use rand::TryRngCore;
use sha2::{Digest, Sha256};
use std::fs::{self as stdfs, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use uuid::Uuid;

const PREFIX: &str = "ocmk1:";
const KEY_LEN: usize = 32;

/// Resolved 32-byte master key and non-secret fingerprint.
#[derive(Debug)]
pub struct MasterKey {
    bytes: SecretBytes,
    fingerprint: String,
}

impl MasterKey {
    /// Raw key bytes. Callers must not persist or log this value.
    #[must_use]
    pub fn bytes(&self) -> &SecretBytes {
        &self.bytes
    }

    /// SHA-256 fingerprint (lowercase hex) stored in `platform_meta`.
    #[must_use]
    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }
}

#[cfg(any(test, feature = "test-support"))]
thread_local! {
    static TEST_ENV: std::cell::RefCell<std::collections::HashMap<String, String>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
}

#[cfg(any(test, feature = "test-support"))]
/// Overlay environment lookup for tests. Never reads or writes the process environment.
pub fn set_test_env(name: &str, value: &str) {
    TEST_ENV.with(|map| {
        map.borrow_mut().insert(name.to_string(), value.to_string());
    });
}

#[cfg(any(test, feature = "test-support"))]
/// Clear the test environment overlay.
pub fn clear_test_env() {
    TEST_ENV.with(|map| map.borrow_mut().clear());
}

fn read_configured_env(name: &str) -> Result<String, PlatformError> {
    #[cfg(any(test, feature = "test-support"))]
    {
        if let Some(value) = TEST_ENV.with(|map| map.borrow().get(name).cloned()) {
            if value.is_empty() {
                return Err(PlatformError::new(
                    ErrorCode::MasterKeyMismatch,
                    "master key environment value is empty",
                ));
            }
            return Ok(value);
        }
    }
    match std::env::var(name) {
        Ok(value) if !value.is_empty() => Ok(value),
        Ok(_) => Err(PlatformError::new(
            ErrorCode::MasterKeyMismatch,
            "master key environment value is empty",
        )),
        Err(std::env::VarError::NotPresent) => Err(PlatformError::new(
            ErrorCode::MasterKeyMismatch,
            "configured master key environment variable is missing",
        )),
        Err(_) => Err(PlatformError::new(
            ErrorCode::MasterKeyMismatch,
            "master key environment value is not valid UTF-8",
        )),
    }
}

/// Resolve the master key from env, file, both, or generate.
pub fn resolve(config: &DataConfig) -> Result<MasterKey, PlatformError> {
    let env_bytes = match &config.master_key_env {
        Some(name) => Some(decode_key(&read_configured_env(name)?)?),
        None => None,
    };

    let file_path = &config.master_key_file;
    fs::require_absolute(file_path)?;
    let file_exists = match stdfs::symlink_metadata(file_path) {
        Ok(_) => true,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => false,
        Err(_) => {
            return Err(PlatformError::new(
                ErrorCode::PathInvalid,
                "master key file is not accessible",
            ));
        }
    };

    let file_bytes = if file_exists {
        Some(read_key_file(file_path)?)
    } else {
        None
    };

    let bytes = match (env_bytes, file_bytes) {
        (Some(env), Some(file)) => {
            if env.expose() != file.expose() {
                return Err(PlatformError::new(
                    ErrorCode::MasterKeyMismatch,
                    "master key env and file values do not match",
                ));
            }
            env
        }
        (Some(env), None) => env,
        (None, Some(file)) => file,
        (None, None) => generate_and_create(file_path)?,
    };
    let fingerprint = fingerprint(bytes.expose());
    Ok(MasterKey { bytes, fingerprint })
}

/// Resolve an already-provisioned master key. Never generates or writes a key file.
pub fn inspect_existing(config: &DataConfig) -> Result<MasterKey, PlatformError> {
    let env_bytes = match &config.master_key_env {
        Some(name) => Some(decode_key(&read_configured_env(name)?)?),
        None => None,
    };
    let file_path = &config.master_key_file;
    fs::require_absolute(file_path)?;
    let file_exists = match stdfs::symlink_metadata(file_path) {
        Ok(_) => true,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => false,
        Err(_) => {
            return Err(PlatformError::new(
                ErrorCode::PathInvalid,
                "master key file is not accessible",
            ));
        }
    };
    let file_bytes = if file_exists {
        Some(read_key_file(file_path)?)
    } else {
        None
    };
    let bytes = match (env_bytes, file_bytes) {
        (Some(env), Some(file)) => {
            if env.expose() != file.expose() {
                return Err(PlatformError::new(
                    ErrorCode::MasterKeyMismatch,
                    "master key env and file values do not match",
                ));
            }
            env
        }
        (Some(env), None) => env,
        (None, None) => {
            return Err(PlatformError::new(
                ErrorCode::MasterKeyMismatch,
                "master key file is missing",
            ));
        }
        (None, Some(file)) => file,
    };
    let fingerprint = fingerprint(bytes.expose());
    Ok(MasterKey { bytes, fingerprint })
}

fn generate_and_create(path: &Path) -> Result<SecretBytes, PlatformError> {
    let parent = path.parent().ok_or_else(|| {
        PlatformError::new(
            ErrorCode::PathInvalid,
            "master key file must have a parent directory",
        )
    })?;
    if !parent.is_dir() {
        return Err(PlatformError::new(
            ErrorCode::PathInvalid,
            "master key parent directory must exist",
        ));
    }
    let mut raw = [0u8; KEY_LEN];
    rand::rngs::OsRng.try_fill_bytes(&mut raw).map_err(|_| {
        PlatformError::new(
            ErrorCode::ConfigInvalid,
            "failed to generate master key material",
        )
    })?;
    let encoded = encode_key(&raw);
    let temp_name = format!(".tmp-master-{}", Uuid::now_v7().as_hyphenated());
    let temp_path = parent.join(temp_name);
    let persist = persist_generated_key(parent, path, &temp_path, encoded.as_bytes());
    if persist.is_err() {
        let _ = stdfs::remove_file(&temp_path);
    }
    match persist {
        Ok(()) => {
            let bytes = SecretBytes::new(raw.to_vec());
            zeroize::Zeroize::zeroize(&mut raw);
            Ok(bytes)
        }
        Err(err) if err.code() == ErrorCode::PathInvalid && path.exists() => {
            zeroize::Zeroize::zeroize(&mut raw);
            read_key_file(path)
        }
        Err(err) => {
            zeroize::Zeroize::zeroize(&mut raw);
            Err(err)
        }
    }
}

fn persist_generated_key(
    parent: &Path,
    final_path: &Path,
    temp_path: &Path,
    encoded: &[u8],
) -> Result<(), PlatformError> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(temp_path)
        .map_err(|_| {
            PlatformError::new(
                ErrorCode::PathInvalid,
                "failed to create master key temp file",
            )
        })?;
    file.write_all(encoded).map_err(|_| {
        PlatformError::new(
            ErrorCode::PathInvalid,
            "failed to write generated master key",
        )
    })?;
    file.sync_all().map_err(|_| {
        PlatformError::new(
            ErrorCode::PathInvalid,
            "failed to fsync generated master key",
        )
    })?;
    drop(file);
    match stdfs::hard_link(temp_path, final_path) {
        Ok(()) => {
            let _ = stdfs::remove_file(temp_path);
            fs::fsync_dir(parent)?;
            Ok(())
        }
        Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
            let _ = stdfs::remove_file(temp_path);
            Err(PlatformError::new(
                ErrorCode::PathInvalid,
                "master key file already exists",
            ))
        }
        Err(_) => Err(PlatformError::new(
            ErrorCode::PathInvalid,
            "failed to install generated master key",
        )),
    }
}

fn read_key_file(path: &Path) -> Result<SecretBytes, PlatformError> {
    let file = fs::open_nofollow(path, false, false).map_err(|_| {
        PlatformError::new(ErrorCode::PathInvalid, "failed to open master key file")
    })?;
    fs::validate_authority_fd(&file)?;
    let mut buf = Vec::new();
    let mut file = file;
    file.read_to_end(&mut buf).map_err(|_| {
        PlatformError::new(ErrorCode::PathInvalid, "failed to read master key file")
    })?;
    let value = std::str::from_utf8(&buf).map_err(|_| {
        PlatformError::new(
            ErrorCode::MasterKeyMismatch,
            "master key file is not valid UTF-8",
        )
    })?;
    if value.as_bytes().iter().any(u8::is_ascii_whitespace) {
        return Err(PlatformError::new(
            ErrorCode::MasterKeyMismatch,
            "master key file must not contain whitespace or extra bytes",
        ));
    }
    decode_key(value)
}

fn decode_key(value: &str) -> Result<SecretBytes, PlatformError> {
    let rest = value.strip_prefix(PREFIX).ok_or_else(|| {
        PlatformError::new(
            ErrorCode::MasterKeyMismatch,
            "master key must use the ocmk1 prefix",
        )
    })?;
    use base64::Engine;
    let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(rest)
        .map_err(|_| {
            PlatformError::new(
                ErrorCode::MasterKeyMismatch,
                "master key is not valid base64url",
            )
        })?;
    if decoded.len() != KEY_LEN {
        return Err(PlatformError::new(
            ErrorCode::MasterKeyMismatch,
            "master key decoded length is invalid",
        ));
    }
    Ok(SecretBytes::new(decoded))
}

fn encode_key(bytes: &[u8; KEY_LEN]) -> String {
    use base64::Engine;
    let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes);
    format!("{PREFIX}{encoded}")
}

fn fingerprint(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    hex_lower(&digest)
}

fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

#[cfg(test)]
pub(crate) fn fingerprint_for_test(bytes: &[u8]) -> String {
    fingerprint(bytes)
}

#[cfg(test)]
#[path = "master_key_tests.rs"]
mod coverage_tests;
