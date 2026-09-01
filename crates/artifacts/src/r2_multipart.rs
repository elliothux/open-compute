//! S3 multipart operations for the typed R2 object store.

use crate::r2::R2ObjectStore;
use crate::r2::{
    create_user_metadata, object_too_large, provider_unavailable, result_unknown, sdk_status,
};
use crate::r2_codec::{integrity_error, millis_datetime, unquote_etag};
use crate::r2_model::{
    R2BucketLocator, R2MultipartCreateOptions, R2ObjectMetadata, R2SsecKey, R2UploadSource,
    R2UploadedPart, UserObjectKey, invalid_options, multipart_invalid, ssec_invalid,
};
use aws_sdk_s3::error::SdkError;
use aws_sdk_s3::operation::abort_multipart_upload::AbortMultipartUploadError;
use aws_sdk_s3::operation::complete_multipart_upload::CompleteMultipartUploadError;
use aws_sdk_s3::operation::create_multipart_upload::CreateMultipartUploadError;
use aws_sdk_s3::operation::upload_part::UploadPartError;
use aws_sdk_s3::primitives::ByteStream;
use aws_sdk_s3::types::{CompletedMultipartUpload, CompletedPart};
use aws_smithy_runtime_api::client::orchestrator::HttpResponse;
use open_compute_core::PlatformError;

impl R2ObjectStore {
    /// List provider multipart ids for one exact tenant key inside its owned bucket prefix.
    pub async fn list_multipart_upload_ids(
        &self,
        locator: &R2BucketLocator,
        key: &UserObjectKey,
    ) -> Result<Vec<String>, PlatformError> {
        let provider_key = self.object_key(locator, key);
        let mut key_marker = None;
        let mut upload_id_marker = None;
        let mut ids = Vec::new();
        for _ in 0..100 {
            let output = self
                .client
                .inner()
                .list_multipart_uploads()
                .bucket(self.client.bucket())
                .prefix(&provider_key)
                .max_uploads(1000)
                .set_key_marker(key_marker.clone())
                .set_upload_id_marker(upload_id_marker.clone())
                .send()
                .await
                .map_err(|_| provider_unavailable())?;
            for upload in output.uploads() {
                if upload.key() == Some(provider_key.as_str()) {
                    ids.push(
                        upload
                            .upload_id()
                            .map(str::to_owned)
                            .ok_or_else(provider_unavailable)?,
                    );
                }
            }
            if !output.is_truncated().unwrap_or(false) {
                ids.sort();
                ids.dedup();
                return Ok(ids);
            }
            let next_key = output.next_key_marker().map(str::to_owned);
            let next_upload = output.next_upload_id_marker().map(str::to_owned);
            if next_key.is_none() || (next_key == key_marker && next_upload == upload_id_marker) {
                return Err(provider_unavailable());
            }
            key_marker = next_key;
            upload_id_marker = next_upload;
        }
        Err(provider_unavailable())
    }

    /// Start a provider multipart upload and return the provider upload id.
    pub async fn create_multipart_upload(
        &self,
        locator: &R2BucketLocator,
        key: &UserObjectKey,
        version: &str,
        options: &R2MultipartCreateOptions,
    ) -> Result<String, PlatformError> {
        let metadata = create_user_metadata(
            version,
            &options.custom_metadata,
            options.storage_class,
            options.ssec.as_ref(),
        )?;
        let mut request = self
            .client
            .inner()
            .create_multipart_upload()
            .bucket(self.client.bucket())
            .key(self.object_key(locator, key))
            .set_metadata(Some(metadata))
            .set_content_type(options.http_metadata.content_type.clone())
            .set_content_language(options.http_metadata.content_language.clone())
            .set_content_disposition(options.http_metadata.content_disposition.clone())
            .set_content_encoding(options.http_metadata.content_encoding.clone())
            .set_cache_control(options.http_metadata.cache_control.clone())
            .set_expires(options.http_metadata.cache_expiry.map(millis_datetime))
            .storage_class(options.storage_class.s3());
        request = apply_ssec_create(request, options.ssec.as_ref());
        let output = request
            .send()
            .await
            .map_err(|error| map_create_failure(&error))?;
        output
            .upload_id()
            .map(str::to_owned)
            .ok_or_else(provider_unavailable)
    }

    /// Upload one already-staged part.
    pub async fn upload_part(
        &self,
        locator: &R2BucketLocator,
        key: &UserObjectKey,
        provider_upload_id: &str,
        part_number: i32,
        source: &R2UploadSource,
        ssec: Option<&R2SsecKey>,
    ) -> Result<R2UploadedPart, PlatformError> {
        if !(1..=crate::r2_model::R2_MAX_MULTIPART_PARTS).contains(&part_number)
            || source.length > crate::r2_model::R2_MAX_MULTIPART_PART_BYTES
        {
            return Err(invalid_options());
        }
        let metadata = std::fs::metadata(&source.path).map_err(|_| provider_unavailable())?;
        if !metadata.file_type().is_file() || metadata.len() != source.length {
            return Err(integrity_error());
        }
        let body = ByteStream::read_from()
            .path(&source.path)
            .length(aws_smithy_types::byte_stream::Length::Exact(source.length))
            .buffer_size(64 * 1024)
            .build()
            .await
            .map_err(|_| provider_unavailable())?;
        let mut request = self
            .client
            .inner()
            .upload_part()
            .bucket(self.client.bucket())
            .key(self.object_key(locator, key))
            .upload_id(provider_upload_id)
            .part_number(part_number)
            .body(body)
            .content_length(i64::try_from(source.length).map_err(|_| object_too_large())?);
        request = apply_ssec_part(request, ssec);
        let output = request
            .send()
            .await
            .map_err(|error| map_upload_part_failure(&error))?;
        let etag = unquote_etag(output.e_tag().ok_or_else(provider_unavailable)?)?;
        Ok(R2UploadedPart { part_number, etag })
    }

    /// Complete a provider multipart upload and verify the resulting object.
    pub async fn complete_multipart_upload(
        &self,
        locator: &R2BucketLocator,
        key: &UserObjectKey,
        provider_upload_id: &str,
        parts: &[R2UploadedPart],
        ssec: Option<&R2SsecKey>,
    ) -> Result<R2ObjectMetadata, PlatformError> {
        if parts.is_empty() {
            return Err(invalid_options());
        }
        let completed = parts
            .iter()
            .map(|part| {
                CompletedPart::builder()
                    .e_tag(format!("\"{}\"", part.etag))
                    .part_number(part.part_number)
                    .build()
            })
            .collect::<Vec<_>>();
        let upload = CompletedMultipartUpload::builder()
            .set_parts(Some(completed))
            .build();
        let mut request = self
            .client
            .inner()
            .complete_multipart_upload()
            .bucket(self.client.bucket())
            .key(self.object_key(locator, key))
            .upload_id(provider_upload_id)
            .multipart_upload(upload);
        request = apply_ssec_complete(request, ssec);
        match request.send().await {
            Ok(_) => {}
            Err(error) if is_ssec_status(sdk_status(&error)) => return Err(ssec_invalid()),
            Err(error) if sdk_status(&error) == Some(404) => return Err(multipart_invalid()),
            Err(error) => return Err(map_complete_failure(&error)),
        }
        self.head(locator, key, ssec)
            .await?
            .ok_or_else(integrity_error)
    }

    /// Abort a provider multipart upload. Missing uploads succeed.
    pub async fn abort_multipart_upload(
        &self,
        locator: &R2BucketLocator,
        key: &UserObjectKey,
        provider_upload_id: &str,
    ) -> Result<(), PlatformError> {
        match self
            .client
            .inner()
            .abort_multipart_upload()
            .bucket(self.client.bucket())
            .key(self.object_key(locator, key))
            .upload_id(provider_upload_id)
            .send()
            .await
        {
            Ok(_) => Ok(()),
            Err(error) if sdk_status(&error) == Some(404) => Ok(()),
            Err(error) => Err(map_abort_failure(&error)),
        }
    }
}

fn apply_ssec_create(
    request: aws_sdk_s3::operation::create_multipart_upload::builders::CreateMultipartUploadFluentBuilder,
    ssec: Option<&R2SsecKey>,
) -> aws_sdk_s3::operation::create_multipart_upload::builders::CreateMultipartUploadFluentBuilder {
    match ssec {
        Some(ssec) => request
            .sse_customer_algorithm("AES256")
            .sse_customer_key(ssec.base64())
            .sse_customer_key_md5(ssec.md5_base64()),
        None => request,
    }
}

fn apply_ssec_part(
    request: aws_sdk_s3::operation::upload_part::builders::UploadPartFluentBuilder,
    ssec: Option<&R2SsecKey>,
) -> aws_sdk_s3::operation::upload_part::builders::UploadPartFluentBuilder {
    match ssec {
        Some(ssec) => request
            .sse_customer_algorithm("AES256")
            .sse_customer_key(ssec.base64())
            .sse_customer_key_md5(ssec.md5_base64()),
        None => request,
    }
}

fn apply_ssec_complete(
    request: aws_sdk_s3::operation::complete_multipart_upload::builders::CompleteMultipartUploadFluentBuilder,
    ssec: Option<&R2SsecKey>,
) -> aws_sdk_s3::operation::complete_multipart_upload::builders::CompleteMultipartUploadFluentBuilder
{
    match ssec {
        Some(ssec) => request
            .sse_customer_algorithm("AES256")
            .sse_customer_key(ssec.base64())
            .sse_customer_key_md5(ssec.md5_base64()),
        None => request,
    }
}

fn is_ssec_status(status: Option<u16>) -> bool {
    matches!(status, Some(400 | 403))
}

fn map_create_failure(error: &SdkError<CreateMultipartUploadError, HttpResponse>) -> PlatformError {
    match sdk_status(error) {
        Some(400 | 403) => ssec_invalid(),
        None => result_unknown(),
        Some(status) if status >= 500 => result_unknown(),
        _ => provider_unavailable(),
    }
}

fn map_upload_part_failure(error: &SdkError<UploadPartError, HttpResponse>) -> PlatformError {
    match sdk_status(error) {
        None => result_unknown(),
        Some(400 | 403) => ssec_invalid(),
        Some(404) => multipart_invalid(),
        Some(status) if status >= 500 => result_unknown(),
        _ => provider_unavailable(),
    }
}

fn map_complete_failure(
    error: &SdkError<CompleteMultipartUploadError, HttpResponse>,
) -> PlatformError {
    if sdk_status(error).is_none() || sdk_status(error).is_some_and(|status| status >= 500) {
        result_unknown()
    } else {
        provider_unavailable()
    }
}

fn map_abort_failure(error: &SdkError<AbortMultipartUploadError, HttpResponse>) -> PlatformError {
    if sdk_status(error).is_none() || sdk_status(error).is_some_and(|status| status >= 500) {
        result_unknown()
    } else {
        provider_unavailable()
    }
}
