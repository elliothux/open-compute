//! Conditional single-part PUT with provider If-Match / If-None-Match fencing.

use super::{
    R2ObjectStore, apply_checksum_metadata, apply_put_checksum, apply_ssec_put, integrity_error,
    is_precondition, is_ssec_denied, map_put_failure, object_too_large, object_user_metadata,
    provider_unavailable, sdk_status, ssec_invalid, validate_upload,
};
use crate::r2_codec::millis_datetime;
use crate::r2_model::{
    R2BucketLocator, R2ObjectMetadata, R2PutOptions, R2SsecKey, R2UploadSource, UserObjectKey,
};
use aws_sdk_s3::primitives::ByteStream;
use base64::Engine as _;
use open_compute_core::PlatformError;

const PUT_CONDITION_ATTEMPTS: u8 = 3;

impl R2ObjectStore {
    /// Upload one already-staged, replayable single-part object and verify it by HEAD.
    pub async fn put_file(
        &self,
        locator: &R2BucketLocator,
        key: &UserObjectKey,
        source: &R2UploadSource,
        options: &R2PutOptions,
        current_ssec: Option<&R2SsecKey>,
    ) -> Result<Option<R2ObjectMetadata>, PlatformError> {
        validate_upload(source, options)?;
        for attempt in 0..PUT_CONDITION_ATTEMPTS {
            let body = ByteStream::read_from()
                .path(&source.path)
                .length(aws_smithy_types::byte_stream::Length::Exact(source.length))
                .buffer_size(64 * 1024)
                .build()
                .await
                .map_err(|_| provider_unavailable())?;
            let mut metadata = object_user_metadata(source, options)?;
            apply_checksum_metadata(&mut metadata, &source.checksums, options.checksum.as_ref());
            let mut request = self
                .client
                .inner()
                .put_object()
                .bucket(self.client.bucket())
                .key(self.object_key(locator, key))
                .body(body)
                .content_length(i64::try_from(source.length).map_err(|_| object_too_large())?)
                .set_metadata(Some(metadata))
                .set_content_type(options.http_metadata.content_type.clone())
                .set_content_language(options.http_metadata.content_language.clone())
                .set_content_disposition(options.http_metadata.content_disposition.clone())
                .set_content_encoding(options.http_metadata.content_encoding.clone())
                .set_cache_control(options.http_metadata.cache_control.clone())
                .set_expires(options.http_metadata.cache_expiry.map(millis_datetime))
                .content_md5(base64::engine::general_purpose::STANDARD.encode(source.checksums.md5))
                .storage_class(options.storage_class.s3());
            request = apply_ssec_put(request, options.ssec.as_ref());
            request = apply_put_checksum(request, options.checksum.as_ref());
            let Some(request) = self
                .apply_put_condition(locator, key, options, current_ssec, request)
                .await?
            else {
                return Ok(None);
            };
            match request.send().await {
                Ok(_) => {
                    return self.verify_put(locator, key, source, options).await;
                }
                Err(error) if is_precondition(&error) => {
                    if attempt + 1 == PUT_CONDITION_ATTEMPTS {
                        return self.final_precondition(locator, key, options).await;
                    }
                }
                Err(error) if is_ssec_denied(sdk_status(&error)) => return Err(ssec_invalid()),
                Err(error) => return Err(map_put_failure(&error)),
            }
        }
        Err(provider_unavailable())
    }

    async fn apply_put_condition(
        &self,
        locator: &R2BucketLocator,
        key: &UserObjectKey,
        options: &R2PutOptions,
        current_ssec: Option<&R2SsecKey>,
        mut request: aws_sdk_s3::operation::put_object::builders::PutObjectFluentBuilder,
    ) -> Result<
        Option<aws_sdk_s3::operation::put_object::builders::PutObjectFluentBuilder>,
        PlatformError,
    > {
        let Some(condition) = &options.only_if else {
            return Ok(Some(request));
        };
        let current = self.head(locator, key, current_ssec).await?;
        match current {
            None => {
                if !condition.matches_missing() {
                    return Ok(None);
                }
                // A missing-object observation must be fenced against creation itself. S3 only
                // defines `*` as the atomic create primitive; the caller's ETag list is
                // re-evaluated after a failed fence on the next bounded attempt.
                request = request.if_none_match("*");
            }
            Some(metadata) => {
                if !condition.matches_object(&metadata.etag, metadata.uploaded) {
                    return Ok(None);
                }
                request = request.if_match(metadata.http_etag);
            }
        }
        Ok(Some(request))
    }

    async fn verify_put(
        &self,
        locator: &R2BucketLocator,
        key: &UserObjectKey,
        source: &R2UploadSource,
        options: &R2PutOptions,
    ) -> Result<Option<R2ObjectMetadata>, PlatformError> {
        let metadata = self
            .head(locator, key, options.ssec.as_ref())
            .await?
            .ok_or_else(integrity_error)?;
        if metadata.size != source.length
            || metadata.version != source.version
            || metadata.checksums != source.checksums.exposed(options.checksum.as_ref())
            || metadata.storage_class != options.storage_class.as_str()
            || metadata.ssec_key_md5.as_deref()
                != options.ssec.as_ref().map(R2SsecKey::md5_base64).as_deref()
        {
            return Err(integrity_error());
        }
        Ok(Some(metadata))
    }

    async fn final_precondition(
        &self,
        locator: &R2BucketLocator,
        key: &UserObjectKey,
        options: &R2PutOptions,
    ) -> Result<Option<R2ObjectMetadata>, PlatformError> {
        let Some(condition) = &options.only_if else {
            return Err(provider_unavailable());
        };
        let current = self.head(locator, key, options.ssec.as_ref()).await?;
        match current {
            None if condition.matches_missing() => Err(provider_unavailable()),
            Some(metadata) if condition.matches_object(&metadata.etag, metadata.uploaded) => {
                Err(provider_unavailable())
            }
            _ => Ok(None),
        }
    }
}
