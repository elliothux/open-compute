use super::*;

impl R2BindingService {
    /// Start one multipart upload through the authenticated management plane.
    pub(crate) async fn management_multipart_create(
        &self,
        instance_id: InstanceId,
        resource_id: ResourceId,
        input: CreateMultipartRequest,
    ) -> Result<Response, PlatformError> {
        let (binding, _bucket, locator, timeout) =
            self.management_multipart_scope(instance_id, resource_id)?;
        let _pin = self.pins.try_pin(resource_id)?;
        self.create_multipart(&binding, &locator, input, timeout)
            .await
    }

    /// Upload one multipart part through the authenticated management plane.
    pub(crate) async fn management_multipart_part(
        &self,
        instance_id: InstanceId,
        resource_id: ResourceId,
        request_id: RequestId,
        header: UploadPartHeader,
        body: Body,
    ) -> Result<Response, PlatformError> {
        let (binding, bucket, locator, timeout) =
            self.management_multipart_scope(instance_id, resource_id)?;
        let _pin = self.pins.try_pin(resource_id)?;
        let _admission = self
            .storage
            .reserve_mutation(bucket.max_object_bytes.saturating_add(64 * 1024))?;
        let _stream = self
            .metrics
            .as_ref()
            .map(|metrics| R2StreamGuard::new(metrics, R2StreamDirection::Upload));
        let lease = self.uploads.acquire(resource_id, timeout).await?;
        let header = serde_json::to_vec(&header).map_err(|_| protocol_error())?;
        let size = u32::try_from(header.len()).map_err(|_| metadata_too_large())?;
        let mut prefix = Vec::with_capacity(4 + header.len());
        prefix.extend_from_slice(&size.to_be_bytes());
        prefix.extend_from_slice(&header);
        let stream =
            futures::stream::once(async move { Ok::<_, axum::Error>(Bytes::from(prefix)) })
                .chain(body.into_data_stream());
        let staged = timeout_result(
            timeout,
            self.stage_part(
                resource_id,
                &request_id.to_string(),
                bucket
                    .max_object_bytes
                    .max(open_compute_artifacts::R2_MIN_MULTIPART_PART_BYTES),
                Body::from_stream(stream),
            ),
        )
        .await?;
        let response = self.upload_part(&binding, &locator, staged, timeout).await;
        drop(lease);
        response
    }

    /// Complete one multipart upload through the authenticated management plane.
    pub(crate) async fn management_multipart_complete(
        &self,
        instance_id: InstanceId,
        resource_id: ResourceId,
        input: CompleteMultipartRequest,
    ) -> Result<Response, PlatformError> {
        let (binding, _bucket, locator, timeout) =
            self.management_multipart_scope(instance_id, resource_id)?;
        let _pin = self.pins.try_pin(resource_id)?;
        self.complete_multipart(&binding, &locator, input, timeout)
            .await
    }

    /// Abort one multipart upload through the authenticated management plane.
    pub(crate) async fn management_multipart_abort(
        &self,
        instance_id: InstanceId,
        resource_id: ResourceId,
        input: AbortMultipartRequest,
    ) -> Result<Response, PlatformError> {
        let (binding, _bucket, locator, timeout) =
            self.management_multipart_scope(instance_id, resource_id)?;
        let _pin = self.pins.try_pin(resource_id)?;
        self.abort_multipart(&binding, &locator, input, timeout)
            .await
    }

    fn management_multipart_scope(
        &self,
        instance_id: InstanceId,
        resource_id: ResourceId,
    ) -> Result<
        (
            AuthorizedBinding,
            open_compute_storage::R2BucketRecord,
            open_compute_artifacts::R2BucketLocator,
            Duration,
        ),
        PlatformError,
    > {
        let binding = crate::resource_binding::management_binding(
            &self.storage,
            instance_id,
            resource_id,
            BindingKind::R2Bucket,
        )?;
        let bucket = R2BucketRepository::new(self.storage.db()).get(instance_id, resource_id)?;
        let locator = self
            .objects
            .locator(bucket.resource.id, &bucket.physical_prefix)?;
        let timeout = Duration::from_millis(self.config.operation_timeout_ms);
        Ok((binding, bucket, locator, timeout))
    }
}
