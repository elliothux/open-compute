use super::*;

pub(super) fn valid_lower_hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

pub(super) fn encode_cursor(key: &ObjectKey) -> String {
    format!(
        "{CURSOR_PREFIX}{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(key.as_str())
    )
}

pub(super) fn decode_cursor(value: &str) -> Result<ObjectKey, BackendError> {
    let encoded = value
        .strip_prefix(CURSOR_PREFIX)
        .ok_or(BackendError::InvalidKey)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(encoded)
        .map_err(|_| BackendError::InvalidKey)?;
    let key = String::from_utf8(bytes).map_err(|_| BackendError::InvalidKey)?;
    let parsed = ObjectKey::new(key)?;
    if encode_cursor(&parsed) != value {
        return Err(BackendError::InvalidKey);
    }
    Ok(parsed)
}

pub(super) fn read_chunk(reader: &mut impl Read, buffer: &mut [u8]) -> Result<usize, BackendError> {
    let mut total = 0;
    while total < buffer.len() {
        let count = reader
            .read(&mut buffer[total..])
            .map_err(|_| BackendError::Unavailable)?;
        if count == 0 {
            break;
        }
        total += count;
    }
    Ok(total)
}

pub(super) fn chunk_nonce(mut base: [u8; 24], index: u64) -> [u8; 24] {
    let index = index.to_be_bytes();
    for (slot, value) in base[16..].iter_mut().zip(index) {
        *slot ^= value;
    }
    base
}

pub(super) fn chunk_aad_from_digest(
    digest: &str,
    object_version: &str,
    plaintext_size: u64,
    index: u64,
) -> Vec<u8> {
    let mut aad = b"open-compute/local-object/chunk/v1".to_vec();
    aad.extend_from_slice(digest.as_bytes());
    aad.extend_from_slice(object_version.as_bytes());
    aad.extend_from_slice(&FORMAT_SCHEMA.to_be_bytes());
    aad.extend_from_slice(&plaintext_size.to_be_bytes());
    aad.extend_from_slice(&index.to_be_bytes());
    aad
}

pub(super) fn verifier_aad(key: &ObjectKey, object_version: &str) -> Vec<u8> {
    verifier_aad_from_digest(
        &hex::encode(Sha256::digest(key.as_str().as_bytes())),
        object_version,
    )
}

pub(super) fn verifier_aad_from_digest(digest: &str, object_version: &str) -> Vec<u8> {
    let mut aad = b"open-compute/local-object/key-verifier/v1".to_vec();
    aad.extend_from_slice(digest.as_bytes());
    aad.extend_from_slice(object_version.as_bytes());
    aad.extend_from_slice(&FORMAT_SCHEMA.to_be_bytes());
    aad
}

pub(super) fn canonical_uuid_v7(value: &str) -> bool {
    uuid::Uuid::parse_str(value)
        .is_ok_and(|id| id.get_version_num() == 7 && id.hyphenated().to_string() == value)
}

pub(super) fn decode_nonce(value: &str) -> Result<[u8; 24], BackendError> {
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(value)
        .map_err(|_| BackendError::Corrupt)?
        .try_into()
        .map_err(|_| BackendError::Corrupt)
}
