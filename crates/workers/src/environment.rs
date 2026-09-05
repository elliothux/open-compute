//! Standard Worker variables and canonical environment validation.

use open_compute_core::{ErrorCode, PlatformError};
use std::collections::BTreeMap;

/// Maximum combined text/JSON variables and secrets per Worker Version.
pub const MAX_VARIABLES: usize = 128;
/// Maximum UTF-8 text/secret value or canonical JSON value bytes.
pub const MAX_VARIABLE_BYTES: usize = 5 * 1024;

/// Canonicalize and validate JSON vars and env names.
#[allow(clippy::type_complexity)]
pub fn canonicalize_vars(
    vars: BTreeMap<String, serde_json::Value>,
) -> Result<
    (
        BTreeMap<String, serde_json::Value>,
        BTreeMap<String, Vec<u8>>,
    ),
    PlatformError,
> {
    if vars.len() > MAX_VARIABLES {
        return Err(PlatformError::new(
            ErrorCode::ResourceLimitExceeded,
            "version contains too many vars",
        ));
    }
    let mut values = BTreeMap::new();
    let mut bytes = BTreeMap::new();
    for (name, value) in vars {
        validate_env_name(&name)?;
        let canonical = canonical_json(value, 0)?;
        let encoded = serde_json::to_vec(&canonical).map_err(|_| {
            PlatformError::new(ErrorCode::BundleInvalid, "var JSON could not be encoded")
        })?;
        let value_bytes = canonical.as_str().map_or(encoded.len(), str::len);
        if value_bytes > MAX_VARIABLE_BYTES {
            return Err(env_too_large());
        }
        values.insert(name.clone(), canonical);
        bytes.insert(name, encoded);
    }
    Ok((values, bytes))
}

/// Validate one P0.2 env name and reject platform/prototype namespaces.
pub fn validate_env_name(name: &str) -> Result<(), PlatformError> {
    let mut bytes = name.bytes();
    let Some(first) = bytes.next() else {
        return Err(invalid_env());
    };
    if !(first.is_ascii_alphabetic() || matches!(first, b'_' | b'$'))
        || bytes.any(|b| !(b.is_ascii_alphanumeric() || matches!(b, b'_' | b'$')))
        || name.starts_with("OPEN_COMPUTE_")
        || name.starts_with("__")
        || name.len() > 128
    {
        return Err(invalid_env());
    }
    Ok(())
}

fn canonical_json(
    value: serde_json::Value,
    depth: usize,
) -> Result<serde_json::Value, PlatformError> {
    if depth > 32 {
        return Err(env_too_large());
    }
    match value {
        serde_json::Value::Array(items) => items
            .into_iter()
            .map(|item| canonical_json(item, depth + 1))
            .collect::<Result<Vec<_>, _>>()
            .map(serde_json::Value::Array),
        serde_json::Value::Object(map) => {
            let mut sorted = BTreeMap::new();
            for (key, value) in map {
                if matches!(key.as_str(), "__proto__" | "prototype" | "constructor") {
                    return Err(PlatformError::new(
                        ErrorCode::BundleInvalid,
                        "var JSON contains a reserved prototype key",
                    ));
                }
                sorted.insert(key, canonical_json(value, depth + 1)?);
            }
            let mut canonical = serde_json::Map::new();
            for (key, value) in sorted {
                canonical.insert(key, value);
            }
            Ok(serde_json::Value::Object(canonical))
        }
        scalar => Ok(scalar),
    }
}

fn invalid_env() -> PlatformError {
    PlatformError::new(
        ErrorCode::BundleInvalid,
        "environment name is invalid or reserved",
    )
}

fn env_too_large() -> PlatformError {
    PlatformError::new(
        ErrorCode::ResourceLimitExceeded,
        "version variables exceed the supported size or depth",
    )
}

#[cfg(test)]
#[path = "environment_tests.rs"]
mod tests;
