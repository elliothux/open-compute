//! Day1 durable structured-clone wire validation for Workflow tenant values.

use super::{WORKFLOW_VALUE_MAX_BYTES, error};
use crate::{ErrorCode, PlatformError};
use base64::Engine as _;

const HEADER: &[u8] = b"OCDV\x01\x02";

/// Decode one canonical standard-base64 Workflow value and verify its profile header and limit.
pub fn durable_value_bytes(input: &str, size_error: ErrorCode) -> Result<Vec<u8>, PlatformError> {
    if input.len() > 1_398_112 || !input.len().is_multiple_of(4) {
        return Err(error(size_error));
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(input)
        .map_err(|_| error(ErrorCode::WorkflowSerializationUnsupported))?;
    if bytes.len() > WORKFLOW_VALUE_MAX_BYTES {
        return Err(error(size_error));
    }
    if !bytes.starts_with(HEADER)
        || base64::engine::general_purpose::STANDARD.encode(&bytes) != input
    {
        return Err(error(ErrorCode::WorkflowSerializationUnsupported));
    }
    Ok(bytes)
}

/// Validate and retain the one authoritative base64 spelling used by JSON control envelopes.
pub fn durable_value_base64(input: &str, size_error: ErrorCode) -> Result<String, PlatformError> {
    durable_value_bytes(input, size_error)?;
    Ok(input.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_only_workflow_profile_and_canonical_base64() {
        assert_eq!(
            durable_value_base64("T0NEVgECAA==", ErrorCode::WorkflowPayloadTooLarge).unwrap(),
            "T0NEVgECAA=="
        );
        for invalid in ["", "T0NEVgEBAA==", "T0NEVgECAA", "T0NEVgECAA==="] {
            assert!(durable_value_base64(invalid, ErrorCode::WorkflowPayloadTooLarge).is_err());
        }
    }
}
