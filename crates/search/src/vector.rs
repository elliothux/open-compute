use crate::{MAX_VECTOR_DIMENSIONS, SearchError};
use std::mem::size_of;

/// Validate one vector against the frozen index dimension count.
pub fn validate_vector(values: &[f32], dimensions: usize) -> Result<(), SearchError> {
    if dimensions == 0 || dimensions > MAX_VECTOR_DIMENSIONS || values.len() != dimensions {
        return Err(SearchError::DimensionMismatch);
    }
    if values.iter().any(|value| !value.is_finite()) {
        return Err(SearchError::NonFiniteVector);
    }
    Ok(())
}

/// Encode validated vector values as authoritative IEEE-754 little-endian f32 bytes.
pub fn encode_f32le(values: &[f32], dimensions: usize) -> Result<Vec<u8>, SearchError> {
    validate_vector(values, dimensions)?;
    let byte_len = dimensions
        .checked_mul(size_of::<f32>())
        .ok_or(SearchError::DimensionMismatch)?;
    let mut encoded = Vec::with_capacity(byte_len);
    for value in values {
        encoded.extend_from_slice(&value.to_le_bytes());
    }
    Ok(encoded)
}

/// Decode authoritative little-endian f32 bytes and reject corruption or non-finite values.
pub fn decode_f32le(bytes: &[u8], dimensions: usize) -> Result<Vec<f32>, SearchError> {
    let expected = dimensions
        .checked_mul(size_of::<f32>())
        .ok_or(SearchError::InvalidVectorEncoding)?;
    if dimensions == 0 || dimensions > MAX_VECTOR_DIMENSIONS || bytes.len() != expected {
        return Err(SearchError::InvalidVectorEncoding);
    }
    let (chunks, remainder) = bytes.as_chunks::<4>();
    debug_assert!(remainder.is_empty());
    let values = chunks
        .iter()
        .map(|chunk| f32::from_le_bytes(*chunk))
        .collect::<Vec<_>>();
    validate_vector(&values, dimensions)?;
    Ok(values)
}
