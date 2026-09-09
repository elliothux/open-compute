use super::*;

pub(super) fn open_local_root(
    path: &std::path::Path,
    create: bool,
) -> Result<OwnedFd, PlatformError> {
    if !path.is_absolute() {
        return Err(platform_integrity(BackendError::Corrupt));
    }
    let names = path
        .components()
        .filter_map(|component| match component {
            std::path::Component::Normal(name) => Some(name.to_os_string()),
            _ => None,
        })
        .collect::<Vec<_>>();
    let mut fd = open(
        "/",
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|_| platform_unavailable())?;
    let last = names.len().saturating_sub(1);
    for (index, name) in names.iter().enumerate() {
        match openat(
            &fd,
            name,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        ) {
            Ok(next) => fd = next,
            Err(error) if create && error == rustix::io::Errno::NOENT && index == last => {
                mkdirat(&fd, name, Mode::RWXU).map_err(|_| platform_unavailable())?;
                fd = openat(
                    &fd,
                    name,
                    OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                    Mode::empty(),
                )
                .map_err(|_| platform_unavailable())?;
            }
            #[cfg(target_os = "macos")]
            Err(error)
                if index == 0
                    && (error == rustix::io::Errno::LOOP || error == rustix::io::Errno::NOTDIR)
                    && matches!(name.as_bytes(), b"var" | b"tmp") =>
            {
                fd = openat(
                    &fd,
                    name,
                    OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
                    Mode::empty(),
                )
                .map_err(|_| platform_integrity(BackendError::Corrupt))?;
            }
            Err(_) => return Err(platform_integrity(BackendError::Corrupt)),
        }
    }
    validate_dir(&fd).map_err(platform_integrity)?;
    Ok(fd)
}

pub(super) fn load_or_initialize_marker(
    root: &OwnedFd,
    config: &LocalObjectStorageConfig,
    platform_id: PlatformId,
) -> Result<FormatMarker, PlatformError> {
    match open_regular(root, FORMAT_FILE) {
        Ok(fd) => {
            drop(fd);
            let marker: FormatMarker =
                read_json_bounded(root, FORMAT_FILE, 64 * 1024).map_err(platform_integrity)?;
            if marker.schema_version != FORMAT_SCHEMA
                || marker.platform_id != platform_id.to_string()
                || marker.prefix != config.prefix
                || marker.r2_prefix != config.r2_prefix
                || !canonical_uuid_v7(&marker.root_id)
            {
                return Err(PlatformError::new(
                    ErrorCode::ObjectStorageAuthorityMismatch,
                    "object storage authority does not match the configured platform",
                ));
            }
            Ok(marker)
        }
        Err(BackendError::NotFound) => {
            for name in dir_names(root).map_err(platform_integrity)? {
                if name != OsStr::new(LOCK_FILE) {
                    return Err(platform_integrity(BackendError::Corrupt));
                }
            }
            let marker = FormatMarker {
                schema_version: FORMAT_SCHEMA,
                platform_id: platform_id.to_string(),
                root_id: uuid::Uuid::now_v7().hyphenated().to_string(),
                prefix: config.prefix.clone(),
                r2_prefix: config.r2_prefix.clone(),
            };
            write_json_create(root, FORMAT_FILE, &marker).map_err(platform_integrity)?;
            fsync(root.as_fd()).map_err(|_| platform_unavailable())?;
            Ok(marker)
        }
        Err(error) => Err(platform_integrity(error)),
    }
}

pub(super) fn authority_sha256(marker: &FormatMarker) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(b"open-compute/object-authority/local/v1");
    for value in [
        marker.schema_version.to_string(),
        marker.root_id.clone(),
        marker.prefix.clone(),
        marker.r2_prefix.clone(),
    ] {
        digest.update((value.len() as u64).to_be_bytes());
        digest.update(value.as_bytes());
    }
    digest.finalize().into()
}

pub(super) fn validate_root_entries(root: &OwnedFd) -> Result<(), BackendError> {
    let mut names = dir_names(root)?;
    names.sort();
    let expected = [LOCK_FILE, FORMAT_FILE, MULTIPART_DIR, OBJECTS_DIR];
    if names.len() != expected.len()
        || names
            .iter()
            .zip(expected)
            .any(|(actual, expected)| actual != OsStr::new(expected))
    {
        return Err(BackendError::Corrupt);
    }
    Ok(())
}

pub(super) fn scan_objects(
    directory: &OwnedFd,
    segments: &[String],
    output: &mut Vec<ListedObject>,
    budget: &mut ScanBudget,
) -> Result<(), BackendError> {
    for name in dir_names(directory)? {
        budget.charge(0)?;
        let Some(name_str) = name.to_str() else {
            return Err(BackendError::Corrupt);
        };
        if name_str.starts_with(".partial-") {
            validate_partial_name(name_str)?;
            continue;
        }
        if name_str == OBJECT_FILE {
            if segments.is_empty() {
                return Err(BackendError::Corrupt);
            }
            let key = ObjectKey::new(segments.join("/"))?;
            let fd = open_regular(directory, OBJECT_FILE)?;
            let stat = fstat(&fd).map_err(|_| BackendError::Unavailable)?;
            budget.charge((stat.st_size as u64).min(HEADER_BYTES as u64))?;
            let metadata = read_header_from_file(&mut File::from(fd), &key)?.metadata;
            output.push(ListedObject { key, metadata });
            continue;
        }
        let mut child_segments = segments.to_owned();
        child_segments.push(name_str.to_owned());
        let candidate = ObjectKey::new(child_segments.join("/"))?;
        let _ = candidate;
        let child = open_child_dir(directory, &name)?;
        scan_objects(&child, &child_segments, output, budget)?;
    }
    Ok(())
}

pub(super) fn remove_object_partials(
    directory: &OwnedFd,
    budget: &mut ScanBudget,
    cutoff_ms: i64,
) -> Result<(), BackendError> {
    for name in dir_names(directory)? {
        budget.charge(0)?;
        let Some(name_str) = name.to_str() else {
            return Err(BackendError::Corrupt);
        };
        if name_str.starts_with(".partial-") {
            validate_partial_name(name_str)?;
            remove_stale_partial(directory, name_str, cutoff_ms, budget)?;
        } else if name_str == OBJECT_FILE {
            let fd = open_regular(directory, name_str)?;
            validate_regular(&fd, None)?;
            let stat = fstat(&fd).map_err(|_| BackendError::Unavailable)?;
            budget.charge((stat.st_size as u64).min(HEADER_BYTES as u64))?;
        } else {
            let child = open_child_dir(directory, &name)?;
            remove_object_partials(&child, budget, cutoff_ms)?;
        }
    }
    fsync(directory.as_fd()).map_err(|_| BackendError::Unavailable)
}

pub(super) fn remove_stale_partial(
    directory: &OwnedFd,
    name: &str,
    cutoff_ms: i64,
    budget: &mut ScanBudget,
) -> Result<(), BackendError> {
    let fd = open_regular(directory, name)?;
    let stat = fstat(&fd).map_err(|_| BackendError::Unavailable)?;
    budget.charge((stat.st_size as u64).min(HEADER_BYTES as u64))?;
    let modified_ms =
        i128::from(stat.st_mtime) * 1_000 + i128::from(stat.st_mtime_nsec) / 1_000_000;
    if modified_ms <= i128::from(cutoff_ms) {
        unlinkat(directory, name, AtFlags::empty()).map_err(|_| BackendError::Unavailable)?;
    }
    Ok(())
}

pub(super) fn validate_manifest(
    manifest: &MultipartManifest,
    key: &ObjectKey,
    upload_id: &str,
    customer_key: Option<&CustomerKey>,
) -> Result<(), BackendError> {
    validate_manifest_record(manifest, upload_id)?;
    if &manifest.key != key {
        return Err(BackendError::MultipartInvalid);
    }
    match (&manifest.encryption, customer_key) {
        (None, None) => Ok(()),
        (None, Some(_)) | (Some(_), None) => Err(BackendError::CustomerKeyInvalid),
        (Some(expected), Some(key_value)) => {
            let actual = encryption_header_with_nonce(
                key,
                key_value,
                &expected.object_version,
                &expected.nonce,
            )?;
            if actual.verifier != expected.verifier || actual.ssec_key_md5 != expected.ssec_key_md5
            {
                return Err(BackendError::CustomerKeyInvalid);
            }
            Ok(())
        }
    }
}

pub(super) fn validate_manifest_record(
    manifest: &MultipartManifest,
    upload_id: &str,
) -> Result<(), BackendError> {
    if manifest.schema_version != FORMAT_SCHEMA
        || manifest.upload_id != upload_id
        || manifest.created_at_ms < 0
        || matches!(
            &manifest.status,
            MultipartStatus::Publishing { etag }
                if etag.is_empty()
                    || etag
                        .bytes()
                        .any(|byte| byte.is_ascii_control() || byte == b'"')
        )
    {
        return Err(BackendError::MultipartInvalid);
    }
    if let Some(encryption) = &manifest.encryption
        && (encryption.algorithm != "xchacha20poly1305-chunked-v1"
            || encryption.chunk_size != CHUNK_BYTES as u32
            || !canonical_uuid_v7(&encryption.object_version)
            || decode_nonce(&encryption.nonce).is_err()
            || base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(&encryption.verifier)
                .is_err()
            || !valid_lower_hex(&encryption.ssec_key_md5, 32))
    {
        return Err(BackendError::MultipartInvalid);
    }
    Ok(())
}
