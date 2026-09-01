//! Durable R2 multipart create/upload/complete/abort with restart reconciliation.

use super::*;
use open_compute_artifacts::{
    R2HttpMetadata, R2MultipartCreateOptions, R2ObjectMetadata, R2SsecKey,
};
use open_compute_core::SecretBytes;
use open_compute_storage::{
    R2MultipartPartRecord, R2MultipartRepository, R2MultipartState, R2MultipartUploadRecord,
    SecretEnvelope,
};
use std::collections::BTreeMap;
#[path = "r2_backend_multipart/reconcile.rs"]
mod reconcile;
pub(crate) use reconcile::reconcile_bucket_multipart;

impl R2BindingService {
    pub(super) async fn stage_part(
        &self,
        resource: ResourceId,
        request_id: &str,
        max_object_bytes: u64,
        body: Body,
    ) -> Result<StagedPart, PlatformError> {
        use futures::TryStreamExt as _;
        let mut stream = body.into_data_stream();
        let mut header_bytes = Vec::with_capacity(4096);
        let mut header_end = None;
        let mut header = None;
        let mut staged = None;
        let mut length = 0_u64;
        let mut reservation = StagingReservation::new(
            self.staging_bytes.clone(),
            self.config.max_staging_bytes,
            self.metrics.clone(),
        );
        while let Some(chunk) = stream.try_next().await.map_err(|_| protocol_error())? {
            let mut remaining = chunk.as_ref();
            while !remaining.is_empty() {
                if header.is_none() {
                    let needed =
                        header_end.map_or(4, |end: usize| end.saturating_sub(header_bytes.len()));
                    let take = needed.min(remaining.len());
                    header_bytes.extend_from_slice(&remaining[..take]);
                    remaining = &remaining[take..];
                    if header_end.is_none() && header_bytes.len() == 4 {
                        let size = u32::from_be_bytes(
                            header_bytes[..4].try_into().map_err(|_| protocol_error())?,
                        );
                        let size = usize::try_from(size).map_err(|_| protocol_error())?;
                        if size > MAX_METADATA_BYTES {
                            return Err(metadata_too_large());
                        }
                        header_end = Some(4_usize.checked_add(size).ok_or_else(protocol_error)?);
                    }
                    if header_end.is_some_and(|end| end == header_bytes.len()) {
                        let parsed: UploadPartHeader = parse_json(&header_bytes[4..])?;
                        let (path, file) = self.staging.create(resource, request_id)?;
                        let file = tokio::fs::File::from_std(file);
                        staged = Some((StagingFile::new(path), file));
                        header = Some(parsed);
                    }
                    continue;
                }
                let added = u64::try_from(remaining.len()).map_err(|_| object_too_large())?;
                length = length.checked_add(added).ok_or_else(object_too_large)?;
                if length > max_object_bytes {
                    return Err(object_too_large());
                }
                reservation.add(added)?;
                ensure_storage_headroom(&self.storage, added)?;
                let (_, file) = staged.as_mut().ok_or_else(protocol_error)?;
                file.write_all(remaining).await.map_err(|_| overloaded())?;
                remaining = &[];
            }
        }
        let header = header.ok_or_else(protocol_error)?;
        let (guard, file) = staged.ok_or_else(protocol_error)?;
        file.sync_all().await.map_err(|_| overloaded())?;
        drop(file);
        Ok(StagedPart {
            header,
            length,
            guard,
            _reservation: reservation,
        })
    }

    pub(super) async fn create_multipart(
        &self,
        binding: &AuthorizedBinding,
        locator: &open_compute_artifacts::R2BucketLocator,
        input: CreateMultipartRequest,
        timeout: Duration,
    ) -> Result<Response, PlatformError> {
        let key = UserObjectKey::parse(&input.key)?;
        let options: R2MultipartCreateOptions = input.options.try_into()?;
        let version = uuid::Uuid::now_v7().hyphenated().to_string();
        let upload_id = uuid::Uuid::now_v7().hyphenated().to_string();
        let now = i64::try_from(unix_ms()?).map_err(|_| protocol_error())?;
        let ssec_key_md5 = options.ssec.as_ref().map(R2SsecKey::md5_base64);
        let ssec_envelope = options
            .ssec
            .as_ref()
            .map(|ssec| seal_ssec(self, binding, &upload_id, ssec))
            .transpose()?;
        let repo = R2MultipartRepository::new(self.storage.db());
        repo.insert_initiating(
            &R2MultipartUploadRecord {
                upload_id: upload_id.clone(),
                resource_id: binding.resource.id,
                account_id: binding.account_id,
                object_key: key.as_str().to_owned(),
                provider_upload_id: None,
                storage_class: options.storage_class.as_str().to_owned(),
                http_metadata: serde_json::to_string(&options.http_metadata)
                    .map_err(|_| protocol_error())?,
                custom_metadata: serde_json::to_string(&options.custom_metadata)
                    .map_err(|_| protocol_error())?,
                ssec_key_md5,
                ssec_envelope,
                object_version: version.clone(),
                completion_manifest: None,
                completed_metadata: None,
                state: R2MultipartState::Initiating,
            },
            now,
        )?;
        let created = mutation_timeout_result(
            timeout,
            self.objects
                .create_multipart_upload(locator, &key, &version, &options),
        )
        .await;
        let provider_upload_id = match created {
            Ok(id) => id,
            Err(error) if error.code() == ErrorCode::R2ResultUnknown => {
                repo.mark_create_unknown(binding.account_id, binding.resource.id, &upload_id, now)?;
                return Err(error);
            }
            Err(error) => {
                let _ = repo.delete_initiating(binding.account_id, binding.resource.id, &upload_id);
                return Err(error);
            }
        };
        if let Err(error) = repo.record_provider_id(
            binding.account_id,
            binding.resource.id,
            &upload_id,
            &provider_upload_id,
            now,
        ) {
            let aborted = mutation_timeout_result(
                timeout,
                self.objects
                    .abort_multipart_upload(locator, &key, &provider_upload_id),
            )
            .await;
            if aborted.is_ok() {
                let _ = repo.delete_initiating(binding.account_id, binding.resource.id, &upload_id);
            } else {
                // Preserve an intent for exact-key provider discovery. A startup pass also
                // handles the case where the provider id was committed despite the returned
                // catalog error.
                let _ = repo.mark_create_unknown(
                    binding.account_id,
                    binding.resource.id,
                    &upload_id,
                    now,
                );
            }
            return Err(error);
        }
        if let Err(error) =
            repo.promote_open(binding.account_id, binding.resource.id, &upload_id, now)
        {
            if let Ok(claimed) =
                repo.claim_for_cleanup(binding.account_id, binding.resource.id, &upload_id, now)
            {
                let _ = self
                    .finish_provider_abort(locator, &key, &claimed, timeout)
                    .await;
            }
            return Err(error);
        }
        Ok(json_response(serde_json::json!({
            "key": key.as_str(),
            "uploadId": upload_id,
        })))
    }

    pub(super) async fn upload_part(
        &self,
        binding: &AuthorizedBinding,
        locator: &open_compute_artifacts::R2BucketLocator,
        staged: StagedPart,
        timeout: Duration,
    ) -> Result<Response, PlatformError> {
        let key = UserObjectKey::parse(&staged.header.key)?;
        let repo = R2MultipartRepository::new(self.storage.db());
        let record = repo
            .get(
                binding.account_id,
                binding.resource.id,
                &staged.header.upload_id,
            )?
            .ok_or_else(multipart_invalid)?;
        if record.object_key != key.as_str() || record.state != R2MultipartState::Open {
            return Err(multipart_invalid());
        }
        let provider_upload_id = record
            .provider_upload_id
            .as_deref()
            .ok_or_else(protocol_error)?;
        let supplied_ssec = parse_ssec(staged.header.ssec_key.as_deref())?;
        let stored_ssec = open_ssec(&self.storage, &record)?;
        if supplied_ssec.as_ref() != stored_ssec.as_ref() {
            return Err(PlatformError::new(
                ErrorCode::R2SsecInvalid,
                "R2 SSE-C key is invalid or does not match the object",
            ));
        }
        let source = R2UploadSource {
            path: staged.guard.path.clone(),
            length: staged.length,
            checksums: hash_file(&staged.guard.path, staged.length)?,
            version: record.object_version.clone(),
        };
        let part = mutation_timeout_result(
            timeout,
            self.objects.upload_part(
                locator,
                &key,
                provider_upload_id,
                staged.header.part_number,
                &source,
                stored_ssec.as_ref(),
            ),
        )
        .await?;
        let now = i64::try_from(unix_ms()?).map_err(|_| protocol_error())?;
        repo.upsert_part(
            binding.account_id,
            binding.resource.id,
            &staged.header.upload_id,
            key.as_str(),
            &R2MultipartPartRecord {
                part_number: part.part_number,
                etag: part.etag.clone(),
                size: staged.length,
            },
            now,
        )?;
        Ok(json_response(part))
    }

    pub(super) async fn complete_multipart(
        &self,
        binding: &AuthorizedBinding,
        locator: &open_compute_artifacts::R2BucketLocator,
        input: CompleteMultipartRequest,
        timeout: Duration,
    ) -> Result<Response, PlatformError> {
        let key = UserObjectKey::parse(&input.key)?;
        let repo = R2MultipartRepository::new(self.storage.db());
        let now = i64::try_from(unix_ms()?).map_err(|_| protocol_error())?;
        let record = repo
            .get(binding.account_id, binding.resource.id, &input.upload_id)?
            .ok_or_else(multipart_invalid)?;
        if record.object_key != key.as_str() {
            return Err(multipart_invalid());
        }
        match record.state {
            R2MultipartState::Completed => {
                let parts = completion_parts(&record, &input.parts)?;
                let stored = repo.list_parts(&input.upload_id)?;
                validate_complete_parts(&parts, &stored)?;
                let metadata = completed_metadata(&record)?;
                validate_completed_object(&record, &parts, &stored, &metadata)?;
                return Ok(json_response(metadata));
            }
            R2MultipartState::Completing => {
                let parts = completion_parts(&record, &input.parts)?;
                let stored = repo.list_parts(&input.upload_id)?;
                validate_complete_parts(&parts, &stored)?;
                return self
                    .reconcile_complete(binding, locator, &record, &parts, timeout)
                    .await;
            }
            R2MultipartState::Open => {}
            R2MultipartState::Initiating
            | R2MultipartState::CreateUnknown
            | R2MultipartState::Aborting
            | R2MultipartState::Aborted => {
                return Err(multipart_invalid());
            }
        }
        let stored = repo.list_parts(&input.upload_id)?;
        validate_complete_parts(&input.parts, &stored)?;
        let completion_manifest = canonical_completion(&input.parts)?;
        let stored_ssec = open_ssec(&self.storage, &record)?;
        self.begin_object_put(binding, &key, &record.object_version, stored_ssec.as_ref())?;
        let record = match repo.begin_complete(
            binding.account_id,
            binding.resource.id,
            &input.upload_id,
            key.as_str(),
            &completion_manifest,
            now,
        ) {
            Ok(record) => record,
            Err(error) => {
                let _ = R2ObjectRepository::new(self.storage.db()).cancel_put(
                    binding.account_id,
                    binding.resource.id,
                    key.as_str(),
                );
                return Err(error);
            }
        };
        self.finish_or_reconcile_complete(binding, locator, &record, &input.parts, timeout)
            .await
    }

    pub(super) async fn abort_multipart(
        &self,
        binding: &AuthorizedBinding,
        locator: &open_compute_artifacts::R2BucketLocator,
        input: AbortMultipartRequest,
        timeout: Duration,
    ) -> Result<Response, PlatformError> {
        let key = UserObjectKey::parse(&input.key)?;
        let repo = R2MultipartRepository::new(self.storage.db());
        let now = i64::try_from(unix_ms()?).map_err(|_| protocol_error())?;
        let record = repo
            .get(binding.account_id, binding.resource.id, &input.upload_id)?
            .ok_or_else(multipart_invalid)?;
        if record.object_key != key.as_str() {
            return Err(multipart_invalid());
        }
        match record.state {
            R2MultipartState::Aborted => return Ok(no_content()),
            R2MultipartState::Aborting => {
                return self
                    .finish_provider_abort(locator, &key, &record, timeout)
                    .await;
            }
            R2MultipartState::Open => {}
            R2MultipartState::Initiating
            | R2MultipartState::CreateUnknown
            | R2MultipartState::Completing
            | R2MultipartState::Completed => {
                return Err(multipart_invalid());
            }
        }
        let record = repo.begin_abort(
            binding.account_id,
            binding.resource.id,
            &input.upload_id,
            key.as_str(),
            now,
        )?;
        self.finish_provider_abort(locator, &key, &record, timeout)
            .await
    }

    async fn reconcile_complete(
        &self,
        binding: &AuthorizedBinding,
        locator: &open_compute_artifacts::R2BucketLocator,
        record: &R2MultipartUploadRecord,
        parts: &[open_compute_artifacts::R2UploadedPart],
        timeout: Duration,
    ) -> Result<Response, PlatformError> {
        let key = UserObjectKey::parse(&record.object_key)?;
        let ssec = open_ssec(&self.storage, record)?;
        if let Some(metadata) =
            timeout_result(timeout, self.objects.head(locator, &key, ssec.as_ref())).await?
        {
            return commit_if_version(self, binding, record, parts, metadata).await;
        }
        if record.state == R2MultipartState::Completed {
            return Err(multipart_invalid());
        }
        self.finish_or_reconcile_complete(binding, locator, record, parts, timeout)
            .await
    }

    async fn finish_or_reconcile_complete(
        &self,
        binding: &AuthorizedBinding,
        locator: &open_compute_artifacts::R2BucketLocator,
        record: &R2MultipartUploadRecord,
        parts: &[open_compute_artifacts::R2UploadedPart],
        timeout: Duration,
    ) -> Result<Response, PlatformError> {
        let key = UserObjectKey::parse(&record.object_key)?;
        let provider_upload_id = record
            .provider_upload_id
            .as_deref()
            .ok_or_else(protocol_error)?;
        let ssec = open_ssec(&self.storage, record)?;
        let result = mutation_timeout_result(
            timeout,
            self.objects.complete_multipart_upload(
                locator,
                &key,
                provider_upload_id,
                parts,
                ssec.as_ref(),
            ),
        )
        .await;
        let now = i64::try_from(unix_ms()?).map_err(|_| protocol_error())?;
        match result {
            Ok(metadata) => commit_if_version(self, binding, record, parts, metadata).await,
            Err(error) if error.code() == ErrorCode::R2ResultUnknown => {
                if let Some(metadata) =
                    timeout_result(timeout, self.objects.head(locator, &key, ssec.as_ref())).await?
                {
                    return commit_if_version(self, binding, record, parts, metadata).await;
                }
                Err(error)
            }
            Err(error) => {
                if let Some(metadata) =
                    timeout_result(timeout, self.objects.head(locator, &key, ssec.as_ref())).await?
                {
                    return commit_if_version(self, binding, record, parts, metadata).await;
                }
                let _ = R2MultipartRepository::new(self.storage.db()).revert_complete(
                    binding.account_id,
                    binding.resource.id,
                    &record.upload_id,
                    &record.object_key,
                    now,
                );
                Err(error)
            }
        }
    }

    async fn finish_provider_abort(
        &self,
        locator: &open_compute_artifacts::R2BucketLocator,
        key: &UserObjectKey,
        record: &R2MultipartUploadRecord,
        timeout: Duration,
    ) -> Result<Response, PlatformError> {
        let Some(provider_upload_id) = record.provider_upload_id.as_deref() else {
            return Err(protocol_error());
        };
        let result = mutation_timeout_result(
            timeout,
            self.objects
                .abort_multipart_upload(locator, key, provider_upload_id),
        )
        .await;
        let now = i64::try_from(unix_ms()?).map_err(|_| protocol_error())?;
        match result {
            Ok(()) => {
                R2MultipartRepository::new(self.storage.db()).finish_abort(
                    record.account_id,
                    record.resource_id,
                    &record.upload_id,
                    &record.object_key,
                    now,
                )?;
                Ok(no_content())
            }
            Err(error) if error.code() == ErrorCode::R2ResultUnknown => Err(error),
            Err(error) => Err(error),
        }
    }
}

async fn commit_if_version(
    service: &R2BindingService,
    binding: &AuthorizedBinding,
    record: &R2MultipartUploadRecord,
    parts: &[open_compute_artifacts::R2UploadedPart],
    metadata: R2ObjectMetadata,
) -> Result<Response, PlatformError> {
    let repo = R2MultipartRepository::new(service.storage.db());
    let stored = repo.list_parts(&record.upload_id)?;
    validate_complete_parts(parts, &stored)?;
    validate_completed_object(record, parts, &stored, &metadata)?;
    let now = i64::try_from(unix_ms()?).map_err(|_| protocol_error())?;
    let key = UserObjectKey::parse(&record.object_key)?;
    service.finish_object_put(binding, &key, &metadata)?;
    if record.state != R2MultipartState::Completed {
        let completed_metadata = serde_json::to_string(&metadata).map_err(|_| protocol_error())?;
        repo.finish_complete(
            binding.account_id,
            binding.resource.id,
            &record.upload_id,
            &record.object_key,
            &completed_metadata,
            now,
        )?;
    }
    Ok(json_response(metadata))
}

fn canonical_completion(
    parts: &[open_compute_artifacts::R2UploadedPart],
) -> Result<String, PlatformError> {
    serde_json::to_string(parts).map_err(|_| protocol_error())
}

fn completion_parts(
    record: &R2MultipartUploadRecord,
    requested: &[open_compute_artifacts::R2UploadedPart],
) -> Result<Vec<open_compute_artifacts::R2UploadedPart>, PlatformError> {
    let raw = record
        .completion_manifest
        .as_deref()
        .ok_or_else(protocol_error)?;
    let persisted: Vec<open_compute_artifacts::R2UploadedPart> =
        serde_json::from_str(raw).map_err(|_| protocol_error())?;
    if persisted.is_empty()
        || canonical_completion(&persisted)? != raw
        || persisted.as_slice() != requested
    {
        return Err(multipart_invalid());
    }
    Ok(persisted)
}

fn completed_metadata(record: &R2MultipartUploadRecord) -> Result<R2ObjectMetadata, PlatformError> {
    let raw = record
        .completed_metadata
        .as_deref()
        .ok_or_else(protocol_error)?;
    let metadata: R2ObjectMetadata = serde_json::from_str(raw).map_err(|_| protocol_error())?;
    if metadata.key != record.object_key || metadata.version != record.object_version {
        return Err(PlatformError::new(
            ErrorCode::R2ObjectMetadataInvalid,
            "R2 object metadata is unavailable",
        ));
    }
    Ok(metadata)
}

fn validate_completed_object(
    record: &R2MultipartUploadRecord,
    requested: &[open_compute_artifacts::R2UploadedPart],
    stored: &[R2MultipartPartRecord],
    metadata: &R2ObjectMetadata,
) -> Result<(), PlatformError> {
    let by_number = stored
        .iter()
        .map(|part| (part.part_number, part))
        .collect::<HashMap<_, _>>();
    let expected_size = requested.iter().try_fold(0_u64, |total, part| {
        total
            .checked_add(
                by_number
                    .get(&part.part_number)
                    .ok_or_else(multipart_invalid)?
                    .size,
            )
            .ok_or_else(completed_metadata_invalid)
    })?;
    let expected_http: R2HttpMetadata =
        serde_json::from_str(&record.http_metadata).map_err(|_| completed_metadata_invalid())?;
    let expected_custom: BTreeMap<String, String> =
        serde_json::from_str(&record.custom_metadata).map_err(|_| completed_metadata_invalid())?;
    let valid_etag = !metadata.etag.is_empty()
        && !metadata
            .etag
            .bytes()
            .any(|byte| byte.is_ascii_control() || byte == b'"')
        && metadata.http_etag == format!("\"{}\"", metadata.etag);
    if metadata.key != record.object_key
        || metadata.version != record.object_version
        || metadata.size != expected_size
        || metadata.range.is_some()
        || metadata.storage_class != record.storage_class
        || metadata.ssec_key_md5 != record.ssec_key_md5
        || metadata.http_metadata.as_ref() != Some(&expected_http)
        || metadata.custom_metadata.as_ref() != Some(&expected_custom)
        || !valid_etag
    {
        return Err(completed_metadata_invalid());
    }
    Ok(())
}

fn completed_metadata_invalid() -> PlatformError {
    PlatformError::new(
        ErrorCode::R2ObjectMetadataInvalid,
        "R2 object metadata is unavailable",
    )
}

fn seal_ssec(
    service: &R2BindingService,
    binding: &AuthorizedBinding,
    upload_id: &str,
    ssec: &R2SsecKey,
) -> Result<String, PlatformError> {
    let envelope = service.storage.crypto().encrypt_r2_ssec(
        &SecretBytes::new(ssec.as_bytes().to_vec()),
        binding.account_id,
        binding.resource.id,
        upload_id,
    )?;
    serde_json::to_string(&envelope).map_err(|_| protocol_error())
}

fn open_ssec(
    storage: &PlatformStorage,
    record: &R2MultipartUploadRecord,
) -> Result<Option<R2SsecKey>, PlatformError> {
    let Some(raw) = record.ssec_envelope.as_deref() else {
        return if record.ssec_key_md5.is_none() {
            Ok(None)
        } else {
            Err(protocol_error())
        };
    };
    let expected_md5 = record.ssec_key_md5.as_deref().ok_or_else(protocol_error)?;
    let envelope: SecretEnvelope = serde_json::from_str(raw).map_err(|_| protocol_error())?;
    let secret = storage.crypto().decrypt_r2_ssec(
        &envelope,
        record.account_id,
        record.resource_id,
        &record.upload_id,
    )?;
    let ssec = R2SsecKey::from_bytes(secret.expose())?;
    if ssec.md5_base64() != expected_md5 {
        return Err(protocol_error());
    }
    Ok(Some(ssec))
}

pub(super) fn validate_complete_parts(
    requested: &[open_compute_artifacts::R2UploadedPart],
    stored: &[R2MultipartPartRecord],
) -> Result<(), PlatformError> {
    if requested.is_empty()
        || requested.len()
            > usize::try_from(open_compute_artifacts::R2_MAX_MULTIPART_PARTS)
                .map_err(|_| protocol_error())?
    {
        return Err(PlatformError::new(
            ErrorCode::R2InvalidOptions,
            "R2 options are invalid",
        ));
    }
    let by_number = stored
        .iter()
        .map(|part| (part.part_number, part))
        .collect::<HashMap<_, _>>();
    let mut previous = 0_i32;
    let mut non_final_size = None;
    let mut total_size = 0_u64;
    for (index, part) in requested.iter().enumerate() {
        if part.part_number <= previous
            || !(1..=open_compute_artifacts::R2_MAX_MULTIPART_PARTS).contains(&part.part_number)
        {
            return Err(PlatformError::new(
                ErrorCode::R2InvalidOptions,
                "R2 options are invalid",
            ));
        }
        let Some(stored_part) = by_number.get(&part.part_number) else {
            return Err(multipart_invalid());
        };
        if stored_part.etag != part.etag {
            return Err(multipart_invalid());
        }
        let last = index + 1 == requested.len();
        if stored_part.size > open_compute_artifacts::R2_MAX_MULTIPART_PART_BYTES
            || (!last
                && (stored_part.size < open_compute_artifacts::R2_MIN_MULTIPART_PART_BYTES
                    || non_final_size.is_some_and(|size| size != stored_part.size)))
        {
            return Err(PlatformError::new(
                ErrorCode::R2InvalidOptions,
                "R2 options are invalid",
            ));
        }
        if !last && non_final_size.is_none() {
            non_final_size = Some(stored_part.size);
        }
        // The fixed Cloudflare limits cap this sum below 50 TiB, far below `u64::MAX`.
        total_size += stored_part.size;
        if total_size > open_compute_artifacts::R2_MAX_MULTIPART_OBJECT_BYTES {
            return Err(PlatformError::new(
                ErrorCode::R2InvalidOptions,
                "R2 options are invalid",
            ));
        }
        previous = part.part_number;
    }
    Ok(())
}

fn multipart_invalid() -> PlatformError {
    PlatformError::new(
        ErrorCode::R2MultipartInvalid,
        "R2 multipart upload is invalid",
    )
}
