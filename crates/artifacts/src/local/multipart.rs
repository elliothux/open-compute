use super::*;

pub(super) fn encryption_header_with_nonce(
    key: &ObjectKey,
    customer_key: &CustomerKey,
    object_version: &str,
    nonce: &str,
) -> Result<EncryptionHeader, BackendError> {
    if !canonical_uuid_v7(object_version) {
        return Err(BackendError::Corrupt);
    }
    let nonce_bytes = decode_nonce(nonce)?;
    let cipher = XChaCha20Poly1305::new(Key::from_slice(customer_key.bytes()));
    let verifier = cipher
        .encrypt(
            XNonce::from_slice(&chunk_nonce(nonce_bytes, u64::MAX)),
            Payload {
                msg: &[],
                aad: &verifier_aad(key, object_version),
            },
        )
        .map_err(|_| BackendError::CustomerKeyInvalid)?;
    Ok(EncryptionHeader {
        algorithm: "xchacha20poly1305-chunked-v1".to_owned(),
        chunk_size: CHUNK_BYTES as u32,
        object_version: object_version.to_owned(),
        nonce: nonce.to_owned(),
        verifier: base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(verifier),
        ssec_key_md5: hex::encode(Md5::digest(customer_key.bytes())),
    })
}

pub(super) fn read_manifest(upload: &OwnedFd) -> Result<MultipartManifest, BackendError> {
    read_json_bounded(upload, MANIFEST_FILE, 64 * 1024)
}

pub(super) fn read_json_bounded<T: DeserializeOwned>(
    parent: &OwnedFd,
    name: &str,
    limit: u64,
) -> Result<T, BackendError> {
    let fd = open_regular(parent, name)?;
    let stat = fstat(&fd).map_err(|_| BackendError::Unavailable)?;
    let size = u64::try_from(stat.st_size).map_err(|_| BackendError::Corrupt)?;
    if size == 0 || size > limit {
        return Err(BackendError::Corrupt);
    }
    let capacity = usize::try_from(size).map_err(|_| BackendError::Corrupt)?;
    let mut bytes = Vec::with_capacity(capacity);
    File::from(fd)
        .read_to_end(&mut bytes)
        .map_err(|_| BackendError::Corrupt)?;
    if bytes.len() != capacity {
        return Err(BackendError::Corrupt);
    }
    serde_json::from_slice(&bytes).map_err(|_| BackendError::Corrupt)
}

pub(super) fn retire_upload_dir(root: &OwnedFd, upload_id: &str) -> Result<(), BackendError> {
    let multipart = open_child_dir(root, MULTIPART_DIR)?;
    let retired_name = format!(".gc-{upload_id}");
    renameat(&multipart, upload_id, &multipart, retired_name.as_str())
        .map_err(|_| BackendError::Unavailable)?;
    fsync(multipart.as_fd()).map_err(|_| BackendError::Unavailable)?;
    remove_retired_upload(&multipart, &retired_name)
}

pub(super) fn remove_retired_upload(
    multipart: &OwnedFd,
    retired_name: &str,
) -> Result<(), BackendError> {
    let upload = open_child_dir(multipart, retired_name)?;
    match open_child_dir(&upload, PARTS_DIR) {
        Ok(parts) => {
            for name in dir_names(&parts)? {
                let Some(name) = name.to_str() else {
                    return Err(BackendError::Corrupt);
                };
                if !valid_part_name(name) && validate_partial_name(name).is_err() {
                    return Err(BackendError::Corrupt);
                }
                let fd = open_regular(&parts, name)?;
                validate_regular(&fd, None)?;
                unlinkat(&parts, name, AtFlags::empty()).map_err(|_| BackendError::Unavailable)?;
            }
            fsync(parts.as_fd()).map_err(|_| BackendError::Unavailable)?;
            unlinkat(&upload, PARTS_DIR, AtFlags::REMOVEDIR)
                .map_err(|_| BackendError::Unavailable)?;
        }
        Err(BackendError::NotFound) => {}
        Err(error) => return Err(error),
    }
    for name in dir_names(&upload)? {
        let Some(name) = name.to_str() else {
            return Err(BackendError::Corrupt);
        };
        if name != MANIFEST_FILE && validate_partial_name(name).is_err() {
            return Err(BackendError::Corrupt);
        }
        let fd = open_regular(&upload, name)?;
        validate_regular(&fd, None)?;
        unlinkat(&upload, name, AtFlags::empty()).map_err(|_| BackendError::Unavailable)?;
    }
    fsync(upload.as_fd()).map_err(|_| BackendError::Unavailable)?;
    unlinkat(multipart, retired_name, AtFlags::REMOVEDIR).map_err(|_| BackendError::Unavailable)?;
    fsync(multipart.as_fd()).map_err(|_| BackendError::Unavailable)
}

pub(super) fn multipart_part_key(
    key: &ObjectKey,
    upload_id: &str,
    part_number: i32,
) -> Result<ObjectKey, BackendError> {
    ObjectKey::new(format!(
        "multipart/{}/{}/{part_number}",
        hex::encode(Sha256::digest(key.as_str().as_bytes())),
        upload_id.replace('-', "")
    ))
}

pub(super) fn validate_upload_id(value: &str) -> Result<&str, BackendError> {
    if !canonical_uuid_v7(value) {
        return Err(BackendError::MultipartInvalid);
    }
    Ok(value)
}

pub(super) fn valid_part_name(value: &str) -> bool {
    parse_part_name(value).is_ok()
}

pub(super) fn parse_part_name(value: &str) -> Result<i32, BackendError> {
    let token = value.strip_suffix(".ocpart").ok_or(BackendError::Corrupt)?;
    let number = token.parse::<i32>().map_err(|_| BackendError::Corrupt)?;
    if !(1..=10_000).contains(&number) || token != number.to_string() {
        return Err(BackendError::Corrupt);
    }
    Ok(number)
}

pub(super) fn validate_partial_name(value: &str) -> Result<(), BackendError> {
    let id = value
        .strip_prefix(".partial-")
        .ok_or(BackendError::Corrupt)?;
    validate_upload_id(id).map(|_| ())
}

pub(super) fn multipart_etag(parts: &[UploadedPart]) -> Result<String, BackendError> {
    let mut binary_etags = Vec::with_capacity(parts.len().saturating_mul(16));
    for part in parts {
        if !valid_lower_hex(&part.etag, 32) {
            return Err(BackendError::MultipartInvalid);
        }
        binary_etags.extend(hex::decode(&part.etag).map_err(|_| BackendError::MultipartInvalid)?);
    }
    Ok(format!(
        "{}-{}",
        hex::encode(Md5::digest(binary_etags)),
        parts.len()
    ))
}

pub(super) fn valid_etag(value: &str) -> bool {
    if valid_lower_hex(value, 32) {
        return true;
    }
    let Some((digest, count)) = value.split_once('-') else {
        return false;
    };
    valid_lower_hex(digest, 32)
        && count
            .parse::<usize>()
            .is_ok_and(|parsed| (1..=10_000).contains(&parsed) && parsed.to_string() == count)
}
