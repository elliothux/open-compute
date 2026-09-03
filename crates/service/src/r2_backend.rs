//! Authorized private data plane for the loaded-isolate R2 facade.

#[path = "r2_backend_multipart.rs"]
pub(crate) mod multipart;
#[path = "r2_backend_objects.rs"]
pub(crate) mod objects;

use crate::metrics::{
    MetricsRegistry, R2Operation, R2ProviderError, R2StreamDirection, R2StreamGuard,
};
use crate::r2_protocol::*;
use axum::body::Body;
use axum::http::{HeaderValue, Method, header};
use axum::response::Response;
use base64::Engine as _;
use bytes::Bytes;
use futures::StreamExt as _;
use open_compute_artifacts::{
    R2GetResult, R2ObjectMetadata, R2ObjectStore, R2UploadSource, UserObjectKey, hash_file,
};
use open_compute_core::{
    AccountId, BindingKind, ErrorCode, OperationClass, PlatformError, R2Config, RequestId,
    ResourceId, VersionId,
};
use open_compute_storage::{
    AuthorizedBinding, BindingRepository, PlatformStorage, R2BucketRepository, R2ObjectListEntry,
    R2ObjectRecord, R2ObjectRepository, R2Staging,
};
use open_compute_workers::{ResourcePin, ResourcePins};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;
use tokio::io::AsyncWriteExt as _;

/// Fully composed R2 binding executor and its bounded host resources.
#[derive(Clone)]
pub struct R2BindingService {
    storage: Arc<PlatformStorage>,
    pins: ResourcePins,
    objects: R2ObjectStore,
    config: R2Config,
    staging: R2Staging,
    uploads: OperationGate,
    downloads: OperationGate,
    staging_bytes: Arc<AtomicU64>,
    metrics: Option<Arc<MetricsRegistry>>,
}

impl std::fmt::Debug for R2BindingService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("R2BindingService")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl R2BindingService {
    /// Bind persisted authority, process pins, the typed store, and local limits.
    pub fn new(
        storage: Arc<PlatformStorage>,
        pins: ResourcePins,
        objects: R2ObjectStore,
        config: R2Config,
    ) -> Result<Self, PlatformError> {
        let staging = R2Staging::open(storage.data_dir().root())?;
        Ok(Self {
            storage,
            pins,
            objects,
            uploads: OperationGate::new(config.max_concurrent_uploads),
            downloads: OperationGate::new(config.max_concurrent_downloads),
            staging_bytes: Arc::new(AtomicU64::new(0)),
            metrics: None,
            config,
            staging,
        })
    }

    /// Attach the process-wide fixed-series R2 metrics registry.
    #[must_use]
    pub fn with_metrics(mut self, metrics: Arc<MetricsRegistry>) -> Self {
        self.metrics = Some(metrics);
        self
    }

    /// Handle one generation-authenticated R2 route from the shared backend.
    pub async fn handle(&self, request: axum::extract::Request) -> Response {
        let operation = metric_operation(request.uri().path());
        let started = std::time::Instant::now();
        let result = Box::pin(self.handle_inner(request)).await;
        if let (Some(metrics), Some(operation)) = (&self.metrics, operation) {
            metrics.observe_r2_operation(operation, result.is_ok(), started.elapsed());
            if let Err(error) = &result {
                metrics.observe_product_error(OperationClass::R2, error.code());
                match error.code() {
                    ErrorCode::R2ProviderUnavailable => {
                        metrics.inc_r2_provider_error(operation, R2ProviderError::Availability);
                    }
                    ErrorCode::R2ObjectMetadataInvalid | ErrorCode::R2PrefixCollision => {
                        metrics.inc_r2_provider_error(operation, R2ProviderError::Integrity);
                    }
                    ErrorCode::R2ResultUnknown => {
                        metrics.inc_r2_provider_error(operation, R2ProviderError::ResultUnknown);
                        metrics.inc_r2_result_unknown(operation == R2Operation::Delete);
                    }
                    _ => {}
                }
            }
        }
        match result {
            Ok(response) => response,
            Err(error) => error_response(&error),
        }
    }

    async fn handle_inner(
        &self,
        request: axum::extract::Request,
    ) -> Result<Response, PlatformError> {
        if request.method() != Method::POST {
            return Err(protocol_error());
        }
        let (binding_id, operation) = parse_path(request.uri().path())?;
        let headers = request.headers();
        let version = parse_header::<VersionId>(headers, "x-open-compute-version-id")?;
        let descriptor = parse_digest(headers)?;
        let request_id = parse_request_id(headers)?;
        if !content_type_matches(headers, operation) {
            return Err(protocol_error());
        }
        let binding = BindingRepository::new(self.storage.db()).authorize(
            binding_id,
            version,
            &descriptor,
        )?;
        validate_binding(&binding, operation)?;
        let mut pin = Some(self.pins.try_pin(binding.resource.id)?);
        let bucket = R2BucketRepository::new(self.storage.db())
            .get(binding.account_id, binding.resource.id)?;
        let locator = self
            .objects
            .locator(bucket.resource.id, &bucket.physical_prefix)?;
        let timeout = Duration::from_millis(self.config.operation_timeout_ms);
        let result = match operation {
            Operation::Head => {
                let body = bounded_json(request.into_body(), MAX_METADATA_BYTES).await?;
                let input: KeyRequest = parse_json(&body)?;
                let key = UserObjectKey::parse(&input.key)?;
                self.authoritative_head(&binding, &locator, &key, timeout)
                    .await?
                    .map_or_else(no_content, json_response)
            }
            Operation::Get => {
                let body = bounded_json(request.into_body(), MAX_METADATA_BYTES).await?;
                let input: GetRequest = parse_json(&body)?;
                let key = UserObjectKey::parse(&input.key)?;
                let ssec = parse_ssec(input.options.ssec_key.as_deref())?;
                let Some((authority, _stored_ssec)) = self
                    .committed_object(&binding, &locator, &key, timeout)
                    .await?
                else {
                    return Ok(no_content());
                };
                if authority.ssec_key_md5.as_deref()
                    != ssec
                        .as_ref()
                        .map(open_compute_artifacts::R2SsecKey::md5_base64)
                        .as_deref()
                {
                    return Err(PlatformError::new(
                        ErrorCode::R2SsecInvalid,
                        "R2 SSE-C key is invalid or does not match the object",
                    ));
                }
                let lease = self.downloads.acquire(bucket.resource.id, timeout).await?;
                match timeout_result(
                    timeout,
                    self.objects.get(
                        &locator,
                        &key,
                        input.options.range,
                        input.options.only_if.as_ref(),
                        ssec.as_ref(),
                    ),
                )
                .await?
                {
                    R2GetResult::Missing => no_content(),
                    R2GetResult::Precondition(metadata) => {
                        objects::validate_object_record(&authority, &metadata)?;
                        if let Some(metrics) = &self.metrics {
                            metrics.inc_r2_condition_failure(false);
                        }
                        framed_metadata(
                            &metadata,
                            None,
                            pin.take().ok_or_else(protocol_error)?,
                            lease,
                            timeout,
                            self.metrics.as_ref(),
                        )?
                    }
                    R2GetResult::Body(download) => {
                        let metadata = download.metadata;
                        objects::validate_object_record(&authority, &metadata)?;
                        let body = download.body;
                        framed_metadata(
                            &metadata,
                            Some(body),
                            pin.take().ok_or_else(protocol_error)?,
                            lease,
                            timeout,
                            self.metrics.as_ref(),
                        )?
                    }
                }
            }
            Operation::Put => {
                let admission = self
                    .storage
                    .reserve_mutation(bucket.max_object_bytes.saturating_add(64 * 1024));
                if let Some(metrics) = &self.metrics {
                    metrics.observe_admission(
                        OperationClass::R2,
                        admission.as_ref().err().map(PlatformError::code),
                    );
                }
                let _admission = admission?;
                let _stream = self
                    .metrics
                    .as_ref()
                    .map(|metrics| R2StreamGuard::new(metrics, R2StreamDirection::Upload));
                let lease = self.uploads.acquire(bucket.resource.id, timeout).await?;
                let staged = timeout_result(
                    timeout,
                    self.stage_put(
                        bucket.resource.id,
                        &request_id,
                        bucket.max_object_bytes,
                        request.into_body(),
                    ),
                )
                .await?;
                let key = UserObjectKey::parse(&staged.header.key)?;
                let source = R2UploadSource {
                    path: staged.guard.path.clone(),
                    length: staged.length,
                    checksums: staged.checksums,
                    version: uuid::Uuid::now_v7().hyphenated().to_string(),
                };
                let options: open_compute_artifacts::R2PutOptions =
                    staged.header.options.try_into()?;
                let current = self
                    .committed_object(&binding, &locator, &key, timeout)
                    .await?;
                self.begin_object_put(&binding, &key, &source.version, options.ssec.as_ref())?;
                let response = mutation_timeout_result(
                    timeout,
                    self.objects.put_file(
                        &locator,
                        &key,
                        &source,
                        &options,
                        current.as_ref().and_then(|(_, ssec)| ssec.as_ref()),
                    ),
                )
                .await;
                drop(lease);
                let response = match response {
                    Ok(Some(metadata)) => {
                        self.finish_object_put(&binding, &key, &metadata)?;
                        Some(metadata)
                    }
                    Ok(None) => {
                        R2ObjectRepository::new(self.storage.db()).cancel_put(
                            binding.account_id,
                            binding.resource.id,
                            key.as_str(),
                        )?;
                        None
                    }
                    Err(error) if error.code() == ErrorCode::R2ResultUnknown => {
                        self.reconcile_object_key(&binding, &locator, &key, timeout)
                            .await?;
                        let metadata = self
                            .authoritative_head(&binding, &locator, &key, timeout)
                            .await?
                            .filter(|metadata| metadata.version == source.version)
                            .ok_or(error)?;
                        Some(metadata)
                    }
                    Err(error) => {
                        R2ObjectRepository::new(self.storage.db()).cancel_put(
                            binding.account_id,
                            binding.resource.id,
                            key.as_str(),
                        )?;
                        return Err(error);
                    }
                };
                if response.is_none()
                    && let Some(metrics) = &self.metrics
                {
                    metrics.inc_r2_condition_failure(true);
                }
                response.map_or_else(no_content, json_response)
            }
            Operation::Delete => {
                let body = bounded_json(request.into_body(), MAX_DELETE_BODY_BYTES).await?;
                let input: DeleteRequest = parse_json(&body)?;
                if input.keys.len() > open_compute_artifacts::R2_MAX_DELETE_KEYS {
                    return Err(PlatformError::new(
                        ErrorCode::R2InvalidOptions,
                        "R2 delete accepts at most 1000 keys",
                    ));
                }
                let mut seen = HashSet::with_capacity(input.keys.len());
                let keys = input
                    .keys
                    .iter()
                    .filter(|key| seen.insert((*key).clone()))
                    .map(|key| UserObjectKey::parse(key))
                    .collect::<Result<Vec<_>, _>>()?;
                let repo = R2ObjectRepository::new(self.storage.db());
                let mut existing = Vec::new();
                for key in &keys {
                    self.ensure_no_object_mutation(&binding, key)?;
                    if repo
                        .get(binding.account_id, binding.resource.id, key.as_str())?
                        .is_some()
                    {
                        existing.push(key.clone());
                    }
                }
                if !existing.is_empty() {
                    let names = existing
                        .iter()
                        .map(|key| key.as_str().to_owned())
                        .collect::<Vec<_>>();
                    repo.begin_delete(
                        binding.account_id,
                        binding.resource.id,
                        &names,
                        i64::try_from(unix_ms()?).map_err(|_| protocol_error())?,
                    )?;
                    match mutation_timeout_result(timeout, self.objects.delete(&locator, &existing))
                        .await
                    {
                        Ok(()) => {
                            repo.finish_delete(binding.account_id, binding.resource.id, &names)?;
                        }
                        Err(error) if error.code() == ErrorCode::R2ResultUnknown => {
                            let mut committed_remains = false;
                            for key in &existing {
                                self.reconcile_object_key(&binding, &locator, key, timeout)
                                    .await?;
                                committed_remains |= repo
                                    .get(binding.account_id, binding.resource.id, key.as_str())?
                                    .is_some();
                            }
                            if committed_remains {
                                return Err(error);
                            }
                        }
                        Err(error) => {
                            for key in &names {
                                repo.cancel_delete(binding.account_id, binding.resource.id, key)?;
                            }
                            return Err(error);
                        }
                    }
                }
                no_content()
            }
            Operation::List => {
                let body = bounded_json(request.into_body(), MAX_METADATA_BYTES).await?;
                let input: ListRequest = parse_json(&body)?;
                self.list(&binding, &locator, input, timeout).await?
            }
            Operation::CreateMultipartUpload => {
                let body = bounded_json(request.into_body(), MAX_METADATA_BYTES).await?;
                let input: CreateMultipartRequest = parse_json(&body)?;
                self.create_multipart(&binding, &locator, input, timeout)
                    .await?
            }
            Operation::UploadPart => {
                let lease = self.uploads.acquire(bucket.resource.id, timeout).await?;
                let staged = timeout_result(
                    timeout,
                    self.stage_part(
                        bucket.resource.id,
                        &request_id,
                        bucket
                            .max_object_bytes
                            .max(open_compute_artifacts::R2_MIN_MULTIPART_PART_BYTES),
                        request.into_body(),
                    ),
                )
                .await?;
                let response = self
                    .upload_part(&binding, &locator, staged, timeout)
                    .await?;
                drop(lease);
                response
            }
            Operation::CompleteMultipartUpload => {
                let body = bounded_json(request.into_body(), MAX_METADATA_BYTES).await?;
                let input: CompleteMultipartRequest = parse_json(&body)?;
                self.complete_multipart(&binding, &locator, input, timeout)
                    .await?
            }
            Operation::AbortMultipartUpload => {
                let body = bounded_json(request.into_body(), MAX_METADATA_BYTES).await?;
                let input: AbortMultipartRequest = parse_json(&body)?;
                self.abort_multipart(&binding, &locator, input, timeout)
                    .await?
            }
        };
        drop(pin);
        Ok(result)
    }

    /// Download one committed object body for an authenticated management request.
    pub(crate) async fn management_object_get(
        &self,
        account_id: AccountId,
        resource_id: ResourceId,
        key: &UserObjectKey,
    ) -> Result<Option<(R2ObjectMetadata, Vec<u8>)>, PlatformError> {
        let binding = crate::resource_binding::management_binding(
            &self.storage,
            account_id,
            resource_id,
            BindingKind::R2Bucket,
        )?;
        let bucket = R2BucketRepository::new(self.storage.db()).get(account_id, resource_id)?;
        let locator = self
            .objects
            .locator(bucket.resource.id, &bucket.physical_prefix)?;
        let timeout = Duration::from_millis(self.config.operation_timeout_ms);
        let Some((authority, ssec)) = self
            .committed_object(&binding, &locator, key, timeout)
            .await?
        else {
            return Ok(None);
        };
        let _pin = self.pins.try_pin(binding.resource.id)?;
        let lease = self.downloads.acquire(bucket.resource.id, timeout).await?;
        let result = timeout_result(
            timeout,
            self.objects.get(&locator, key, None, None, ssec.as_ref()),
        )
        .await?;
        drop(lease);
        match result {
            R2GetResult::Missing => Ok(None),
            R2GetResult::Precondition(metadata) => {
                objects::validate_object_record(&authority, &metadata)?;
                Ok(Some((metadata, Vec::new())))
            }
            R2GetResult::Body(download) => {
                objects::validate_object_record(&authority, &download.metadata)?;
                let bytes =
                    read_object_bytes(download.body, download.metadata.size, timeout).await?;
                Ok(Some((download.metadata, bytes)))
            }
        }
    }

    /// Replace one object from a bounded authenticated management request body.
    pub(crate) async fn management_object_put(
        &self,
        account_id: AccountId,
        resource_id: ResourceId,
        key: &UserObjectKey,
        request_id: RequestId,
        options: open_compute_artifacts::R2PutOptions,
        expected_length: Option<u64>,
        body: Body,
    ) -> Result<Option<R2ObjectMetadata>, PlatformError> {
        let binding = crate::resource_binding::management_binding(
            &self.storage,
            account_id,
            resource_id,
            BindingKind::R2Bucket,
        )?;
        let bucket = R2BucketRepository::new(self.storage.db()).get(account_id, resource_id)?;
        let locator = self
            .objects
            .locator(bucket.resource.id, &bucket.physical_prefix)?;
        let timeout = Duration::from_millis(self.config.operation_timeout_ms);
        let admission = self
            .storage
            .reserve_mutation(bucket.max_object_bytes.saturating_add(64 * 1024))?;
        let _admission = admission;
        let _stream = self
            .metrics
            .as_ref()
            .map(|metrics| R2StreamGuard::new(metrics, R2StreamDirection::Upload));
        let lease = self.uploads.acquire(bucket.resource.id, timeout).await?;
        let staged = timeout_result(
            timeout,
            self.stage_management_put(
                bucket.resource.id,
                &request_id.to_string(),
                key.as_str(),
                bucket.max_object_bytes,
                body,
            ),
        )
        .await?;
        if expected_length.is_some_and(|length| length != staged.length) {
            return Err(PlatformError::new(
                ErrorCode::ConfigInvalid,
                "R2 object Content-Length does not match the request body",
            ));
        }
        let source = R2UploadSource {
            path: staged.guard.path.clone(),
            length: staged.length,
            checksums: staged.checksums,
            version: uuid::Uuid::now_v7().hyphenated().to_string(),
        };
        let current = self
            .committed_object(&binding, &locator, key, timeout)
            .await?;
        self.begin_object_put(&binding, key, &source.version, options.ssec.as_ref())?;
        let response = mutation_timeout_result(
            timeout,
            self.objects.put_file(
                &locator,
                key,
                &source,
                &options,
                current.as_ref().and_then(|(_, ssec)| ssec.as_ref()),
            ),
        )
        .await;
        drop(lease);
        match response {
            Ok(Some(metadata)) => {
                self.finish_object_put(&binding, key, &metadata)?;
                Ok(Some(metadata))
            }
            Ok(None) => {
                R2ObjectRepository::new(self.storage.db()).cancel_put(
                    binding.account_id,
                    binding.resource.id,
                    key.as_str(),
                )?;
                Ok(None)
            }
            Err(error) if error.code() == ErrorCode::R2ResultUnknown => {
                self.reconcile_object_key(&binding, &locator, key, timeout)
                    .await?;
                Ok(self
                    .authoritative_head(&binding, &locator, key, timeout)
                    .await?
                    .filter(|metadata| metadata.version == source.version))
            }
            Err(error) => {
                R2ObjectRepository::new(self.storage.db()).cancel_put(
                    binding.account_id,
                    binding.resource.id,
                    key.as_str(),
                )?;
                Err(error)
            }
        }
    }

    /// Delete one committed object for an authenticated management request.
    pub(crate) async fn management_object_delete(
        &self,
        account_id: AccountId,
        resource_id: ResourceId,
        key: &UserObjectKey,
    ) -> Result<bool, PlatformError> {
        let binding = crate::resource_binding::management_binding(
            &self.storage,
            account_id,
            resource_id,
            BindingKind::R2Bucket,
        )?;
        let bucket = R2BucketRepository::new(self.storage.db()).get(account_id, resource_id)?;
        let locator = self
            .objects
            .locator(bucket.resource.id, &bucket.physical_prefix)?;
        let timeout = Duration::from_millis(self.config.operation_timeout_ms);
        let repo = R2ObjectRepository::new(self.storage.db());
        self.ensure_no_object_mutation(&binding, key)?;
        if repo.get(account_id, resource_id, key.as_str())?.is_none() {
            return Ok(false);
        }
        let names = vec![key.as_str().to_owned()];
        repo.begin_delete(
            account_id,
            resource_id,
            &names,
            i64::try_from(unix_ms()?).map_err(|_| protocol_error())?,
        )?;
        match mutation_timeout_result(
            timeout,
            self.objects.delete(&locator, std::slice::from_ref(key)),
        )
        .await
        {
            Ok(()) => {
                repo.finish_delete(account_id, resource_id, &names)?;
                Ok(true)
            }
            Err(error) if error.code() == ErrorCode::R2ResultUnknown => {
                self.reconcile_object_key(&binding, &locator, key, timeout)
                    .await?;
                if repo.get(account_id, resource_id, key.as_str())?.is_some() {
                    return Err(error);
                }
                Ok(true)
            }
            Err(error) => {
                repo.cancel_delete(account_id, resource_id, key.as_str())?;
                Err(error)
            }
        }
    }

    async fn stage_management_put(
        &self,
        resource: ResourceId,
        request_id: &str,
        key: &str,
        max_object_bytes: u64,
        body: Body,
    ) -> Result<StagedPut, PlatformError> {
        use futures::TryStreamExt as _;
        let mut stream = body.into_data_stream();
        let (path, file) = self.staging.create(resource, request_id)?;
        let mut file = tokio::fs::File::from_std(file);
        let guard = StagingFile::new(path);
        let mut length = 0_u64;
        let mut reservation = StagingReservation::new(
            self.staging_bytes.clone(),
            self.config.max_staging_bytes,
            self.metrics.clone(),
        );
        while let Some(chunk) = stream.try_next().await.map_err(|_| protocol_error())? {
            let added = u64::try_from(chunk.len()).map_err(|_| object_too_large())?;
            length = length.checked_add(added).ok_or_else(object_too_large)?;
            if length > max_object_bytes {
                return Err(object_too_large());
            }
            reservation.add(added)?;
            ensure_storage_headroom(&self.storage, added)?;
            file.write_all(&chunk).await.map_err(|_| overloaded())?;
        }
        file.sync_all().await.map_err(|_| overloaded())?;
        drop(file);
        let checksums = hash_file(&guard.path, length)?;
        Ok(StagedPut {
            header: PutHeader {
                key: key.to_owned(),
                options: PutWireOptions::default(),
            },
            length,
            checksums,
            guard,
            _reservation: reservation,
        })
    }

    async fn stage_put(
        &self,
        resource: ResourceId,
        request_id: &str,
        max_object_bytes: u64,
        body: Body,
    ) -> Result<StagedPut, PlatformError> {
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
                        let parsed: PutHeader = parse_json(&header_bytes[4..])?;
                        parsed.options.validate()?;
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
        let checksums = hash_file(&guard.path, length)?;
        Ok(StagedPut {
            header,
            length,
            checksums,
            guard,
            _reservation: reservation,
        })
    }

    async fn list(
        &self,
        binding: &AuthorizedBinding,
        locator: &open_compute_artifacts::R2BucketLocator,
        input: ListRequest,
        timeout: Duration,
    ) -> Result<Response, PlatformError> {
        input.validate()?;
        let include_mask = include_mask(&input.include)?;
        let cursor_after = match input.cursor.as_deref() {
            Some(cursor) => Some(self.decode_cursor(binding, &input, include_mask, cursor)?),
            None => None,
        };
        let limit = input.limit.max(1);
        let after = cursor_after
            .as_ref()
            .and_then(|value| value.as_deref())
            .or(input.start_after.as_deref());
        let page = R2ObjectRepository::new(self.storage.db()).list(
            binding.account_id,
            binding.resource.id,
            &input.prefix,
            input.delimiter.as_deref(),
            after,
            limit,
        )?;
        if input.limit == 0 {
            let truncated = !page.entries.is_empty();
            let cursor = truncated
                .then(|| {
                    self.encode_cursor(binding, &input, include_mask, after.map(str::to_owned))
                })
                .transpose()?;
            return Ok(json_response(ListResponse {
                objects: Vec::new(),
                truncated,
                cursor,
                delimited_prefixes: Vec::new(),
            }));
        }
        let (objects, delimited_prefixes) = page.entries.into_iter().fold(
            (Vec::new(), Vec::new()),
            |(mut objects, mut prefixes), entry| {
                match entry {
                    R2ObjectListEntry::Object(object) => objects.push(object),
                    R2ObjectListEntry::DelimitedPrefix(prefix) => prefixes.push(prefix),
                }
                (objects, prefixes)
            },
        );
        if let Some(metrics) = &self.metrics {
            metrics.add_r2_list_head_fanout(objects.len() as u64);
        }
        let headed = self
            .head_list_objects(binding, locator, &objects, timeout)
            .await?;
        let objects = headed
            .into_iter()
            .map(|metadata| list_object_json(metadata, include_mask))
            .collect::<Vec<_>>();
        let cursor = page
            .next_after
            .map(|after_key| self.encode_cursor(binding, &input, include_mask, Some(after_key)))
            .transpose()?;
        Ok(json_response(ListResponse {
            objects,
            truncated: cursor.is_some(),
            cursor,
            delimited_prefixes,
        }))
    }

    async fn head_list_objects(
        &self,
        binding: &AuthorizedBinding,
        locator: &open_compute_artifacts::R2BucketLocator,
        objects: &[R2ObjectRecord],
        timeout: Duration,
    ) -> Result<Vec<R2ObjectMetadata>, PlatformError> {
        use futures::{StreamExt as _, stream};
        let fanout = usize::try_from(self.config.max_metadata_head_concurrency)
            .unwrap_or(1)
            .max(1);
        let owned = objects.to_vec();
        let results = stream::iter(owned.into_iter().enumerate().map(
            |(index, object)| async move {
                let key = UserObjectKey::parse(&object.object_key)?;
                let metadata = self
                    .authoritative_head(binding, locator, &key, timeout)
                    .await?
                    .ok_or_else(metadata_invalid)?;
                Ok::<_, PlatformError>((index, metadata))
            },
        ))
        .buffer_unordered(fanout)
        .collect::<Vec<_>>()
        .await;
        let mut ordered: Vec<Option<R2ObjectMetadata>> = (0..objects.len()).map(|_| None).collect();
        for result in results {
            let (index, value) = result?;
            ordered[index] = Some(value);
        }
        ordered
            .into_iter()
            .map(|value| value.ok_or_else(protocol_error))
            .collect()
    }

    fn encode_cursor(
        &self,
        binding: &AuthorizedBinding,
        input: &ListRequest,
        include_mask: u8,
        after_key: Option<String>,
    ) -> Result<String, PlatformError> {
        let now = unix_ms()?;
        let payload = CursorPayload {
            v: 1,
            resource_id: binding.resource.id,
            generation: binding.resource.spec_generation,
            prefix_sha256: digest_text(&input.prefix),
            delimiter_sha256: digest_text(input.delimiter.as_deref().unwrap_or("")),
            include_mask,
            start_after_sha256: digest_text(input.start_after.as_deref().unwrap_or("")),
            after_key,
            expires_at_ms: now.saturating_add(self.config.cursor_ttl_ms),
        };
        let bytes = serde_json::to_vec(&payload).map_err(|_| cursor_invalid())?;
        let signature = self.storage.crypto().sign_r2_cursor(&bytes);
        let base64 = base64::engine::general_purpose::URL_SAFE_NO_PAD;
        Ok(format!(
            "{}.{}",
            base64.encode(bytes),
            base64.encode(signature)
        ))
    }

    fn decode_cursor(
        &self,
        binding: &AuthorizedBinding,
        input: &ListRequest,
        include_mask: u8,
        cursor: &str,
    ) -> Result<Option<String>, PlatformError> {
        let (payload, signature) = cursor.split_once('.').ok_or_else(cursor_invalid)?;
        if signature.contains('.') {
            return Err(cursor_invalid());
        }
        let base64 = base64::engine::general_purpose::URL_SAFE_NO_PAD;
        let payload = base64.decode(payload).map_err(|_| cursor_invalid())?;
        let signature = base64.decode(signature).map_err(|_| cursor_invalid())?;
        if !self.storage.crypto().verify_r2_cursor(&payload, &signature) {
            return Err(cursor_invalid());
        }
        let decoded: CursorPayload =
            serde_json::from_slice(&payload).map_err(|_| cursor_invalid())?;
        if decoded.v != 1
            || decoded.resource_id != binding.resource.id
            || decoded.generation != binding.resource.spec_generation
            || decoded.prefix_sha256 != digest_text(&input.prefix)
            || decoded.delimiter_sha256 != digest_text(input.delimiter.as_deref().unwrap_or(""))
            || decoded.include_mask != include_mask
            || decoded.start_after_sha256 != digest_text(input.start_after.as_deref().unwrap_or(""))
            || decoded.expires_at_ms < unix_ms()?
        {
            return Err(cursor_invalid());
        }
        Ok(decoded.after_key)
    }
}

struct StagedPut {
    header: PutHeader,
    length: u64,
    checksums: open_compute_artifacts::R2ComputedChecksums,
    guard: StagingFile,
    _reservation: StagingReservation,
}

struct StagedPart {
    header: UploadPartHeader,
    length: u64,
    guard: StagingFile,
    _reservation: StagingReservation,
}

struct StagingFile {
    path: std::path::PathBuf,
}
impl StagingFile {
    fn new(path: std::path::PathBuf) -> Self {
        Self { path }
    }
}
impl Drop for StagingFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
        if let Some(parent) = self.path.parent() {
            let _ = std::fs::remove_dir(parent);
        }
    }
}

struct StagingReservation {
    used: Arc<AtomicU64>,
    max: u64,
    bytes: u64,
    metrics: Option<Arc<MetricsRegistry>>,
}
impl StagingReservation {
    fn new(used: Arc<AtomicU64>, max: u64, metrics: Option<Arc<MetricsRegistry>>) -> Self {
        Self {
            used,
            max,
            bytes: 0,
            metrics,
        }
    }
    fn add(&mut self, bytes: u64) -> Result<(), PlatformError> {
        let mut current = self.used.load(Ordering::Acquire);
        loop {
            let next = current.checked_add(bytes).ok_or_else(overloaded)?;
            if next > self.max {
                return Err(overloaded());
            }
            match self.used.compare_exchange_weak(
                current,
                next,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => {
                    self.bytes = self.bytes.saturating_add(bytes);
                    if let Some(metrics) = &self.metrics {
                        metrics.adjust_r2_staging_bytes(bytes, true);
                        metrics.add_r2_bytes(R2StreamDirection::Upload, bytes);
                    }
                    return Ok(());
                }
                Err(found) => current = found,
            }
        }
    }
}
impl Drop for StagingReservation {
    fn drop(&mut self) {
        self.used.fetch_sub(self.bytes, Ordering::AcqRel);
        if let Some(metrics) = &self.metrics {
            metrics.adjust_r2_staging_bytes(self.bytes, false);
        }
    }
}

#[derive(Clone)]
struct OperationGate {
    global: Arc<tokio::sync::Semaphore>,
    per_resource: usize,
    resources: Arc<Mutex<HashMap<ResourceId, Weak<tokio::sync::Semaphore>>>>,
}
impl OperationGate {
    fn new(limit: u32) -> Self {
        let limit = limit.max(1) as usize;
        Self {
            global: Arc::new(tokio::sync::Semaphore::new(limit)),
            per_resource: limit,
            resources: Arc::new(Mutex::new(HashMap::new())),
        }
    }
    async fn acquire(
        &self,
        resource: ResourceId,
        timeout: Duration,
    ) -> Result<OperationLease, PlatformError> {
        let global = tokio::time::timeout(timeout, self.global.clone().acquire_owned())
            .await
            .map_err(|_| overloaded())?
            .map_err(|_| overloaded())?;
        let gate = {
            let mut resources = self
                .resources
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            resources.retain(|_, gate| gate.strong_count() > 0);
            resources
                .get(&resource)
                .and_then(Weak::upgrade)
                .unwrap_or_else(|| {
                    let gate = Arc::new(tokio::sync::Semaphore::new(self.per_resource));
                    resources.insert(resource, Arc::downgrade(&gate));
                    gate
                })
        };
        let resource = tokio::time::timeout(timeout, gate.acquire_owned())
            .await
            .map_err(|_| overloaded())?
            .map_err(|_| overloaded())?;
        Ok(OperationLease {
            _global: global,
            _resource: resource,
        })
    }
}
struct OperationLease {
    _global: tokio::sync::OwnedSemaphorePermit,
    _resource: tokio::sync::OwnedSemaphorePermit,
}

fn framed_metadata(
    metadata: &R2ObjectMetadata,
    body: Option<aws_sdk_s3::primitives::ByteStream>,
    pin: ResourcePin,
    lease: OperationLease,
    timeout: Duration,
    metrics: Option<&Arc<MetricsRegistry>>,
) -> Result<Response, PlatformError> {
    let has_body = body.is_some();
    let expected = metadata
        .range
        .and_then(|range| range.length)
        .unwrap_or(metadata.size);
    let header_bytes =
        serde_json::to_vec(&serde_json::json!({"meta": metadata, "hasBody": has_body}))
            .map_err(|_| protocol_error())?;
    if header_bytes.len() > MAX_METADATA_BYTES {
        return Err(metadata_too_large());
    }
    let mut prefix = u32::try_from(header_bytes.len())
        .map_err(|_| protocol_error())?
        .to_be_bytes()
        .to_vec();
    prefix.extend_from_slice(&header_bytes);
    let mut response = if let Some(body) = body {
        struct State {
            body: aws_sdk_s3::primitives::ByteStream,
            remaining: u64,
            deadline: tokio::time::Instant,
            failed: bool,
            metrics: Option<Arc<MetricsRegistry>>,
            _stream: Option<R2StreamGuard>,
            _pin: ResourcePin,
            _lease: OperationLease,
        }
        let stream_metrics = metrics.cloned();
        let active = stream_metrics
            .as_ref()
            .map(|metrics| R2StreamGuard::new(metrics, R2StreamDirection::Download));
        let stream = futures::stream::unfold(
            State {
                body,
                remaining: expected,
                deadline: tokio::time::Instant::now() + timeout,
                failed: false,
                metrics: stream_metrics,
                _stream: active,
                _pin: pin,
                _lease: lease,
            },
            |mut state| async move {
                if state.failed {
                    return None;
                }
                match tokio::time::timeout_at(state.deadline, state.body.next()).await {
                    Ok(Some(Ok(bytes)))
                        if u64::try_from(bytes.len())
                            .ok()
                            .is_some_and(|size| size <= state.remaining) =>
                    {
                        let count = bytes.len() as u64;
                        state.remaining -= count;
                        if let Some(metrics) = &state.metrics {
                            metrics.add_r2_bytes(R2StreamDirection::Download, count);
                        }
                        Some((Ok::<Bytes, std::io::Error>(bytes), state))
                    }
                    Ok(None) if state.remaining == 0 => None,
                    _ => {
                        state.failed = true;
                        Some((
                            Err(std::io::Error::other(
                                ErrorCode::R2ProviderUnavailable.as_str(),
                            )),
                            state,
                        ))
                    }
                }
            },
        );
        let stream =
            futures::stream::once(async move { Ok::<Bytes, std::io::Error>(Bytes::from(prefix)) })
                .chain(stream);
        Response::new(Body::from_stream(stream))
    } else {
        drop(pin);
        drop(lease);
        Response::new(Body::from(prefix))
    };
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(FRAME_CONTENT_TYPE),
    );
    Ok(response)
}

async fn read_object_bytes(
    body: aws_sdk_s3::primitives::ByteStream,
    expected_size: u64,
    timeout: Duration,
) -> Result<Vec<u8>, PlatformError> {
    use aws_sdk_s3::primitives::AggregatedBytes;
    let collected: AggregatedBytes = tokio::time::timeout(timeout, body.collect())
        .await
        .map_err(|_| protocol_error())?
        .map_err(|_| protocol_error())?;
    let bytes = collected.into_bytes().to_vec();
    if u64::try_from(bytes.len()).map_err(|_| object_too_large())? != expected_size {
        return Err(metadata_invalid());
    }
    Ok(bytes)
}

#[cfg(test)]
#[path = "r2_backend_tests.rs"]
mod tests;
