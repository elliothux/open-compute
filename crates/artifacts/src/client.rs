//! Production AWS S3 client construction.

use crate::backend::{
    BackendError, CustomerKey, GetOptions, HeadOptions, ListPage, ListedObject,
    OBJECT_KEY_MAX_BYTES, ObjectBody, ObjectGet, ObjectKey, ObjectMetadata, ObjectRange,
    ObjectSource, ObjectStorageClass, PutMode, PutOptions, UploadedPart,
};
use crate::credentials::S3Credentials;
use aws_credential_types::Credentials;
use aws_sdk_s3::Client;
use aws_sdk_s3::config::{
    BehaviorVersion, Region, RequestChecksumCalculation, ResponseChecksumValidation,
    retry::RetryConfig, timeout::TimeoutConfig,
};
use aws_sdk_s3::error::SdkError;
use aws_sdk_s3::primitives::{ByteStream, DateTime};
use aws_sdk_s3::types::{
    CompletedMultipartUpload, CompletedPart, Delete, ObjectIdentifier,
    ObjectStorageClass as S3ObjectStorageClass, StorageClass,
};
use aws_smithy_http_client::Builder as HttpBuilder;
use aws_smithy_http_client::tls::rustls_provider::CryptoMode;
use aws_smithy_http_client::tls::{Provider as TlsProvider, TlsContext, TrustStore};
use aws_smithy_runtime_api::client::orchestrator::HttpResponse;
use base64::Engine as _;
use futures::Stream;
use md5::{Digest as _, Md5};
use open_compute_core::{ErrorCode, PlatformError, S3Config};
use sha2::Sha256;
use std::collections::BTreeMap;
use std::io::{Seek as _, SeekFrom};
use std::os::unix::fs::FileExt as _;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

/// AWS SDK response body kept entirely inside the S3 adapter boundary.
pub(crate) struct S3ObjectBody(ByteStream);

impl S3ObjectBody {
    const fn new(body: ByteStream) -> Self {
        Self(body)
    }
}

impl Stream for S3ObjectBody {
    type Item = Result<bytes::Bytes, std::io::Error>;

    fn poll_next(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Pin::new(&mut self.0).poll_next(context).map(|item| {
            item.map(|result| {
                result.map_err(|_| std::io::Error::other("object body stream failed"))
            })
        })
    }
}

/// Configured production S3 client plus bucket/prefix context.
#[derive(Debug, Clone)]
pub(crate) struct S3Backend {
    inner: Client,
    bucket: String,
    prefix: String,
    r2_prefix: String,
    authority_sha256: [u8; 32],
    max_artifact_bytes: u64,
}

impl S3Backend {
    /// Build a `SigV4` client from validated config and resolved credentials.
    pub(crate) fn connect(
        config: &S3Config,
        credentials: &S3Credentials,
        max_artifact_bytes: u64,
    ) -> Result<Self, PlatformError> {
        if !config.verify_tls {
            return Err(PlatformError::new(
                ErrorCode::ConfigInvalid,
                "s3.verify_tls cannot be disabled",
            ));
        }
        if max_artifact_bytes == 0 {
            return Err(PlatformError::new(
                ErrorCode::LimitInvalid,
                "configured limit must be greater than zero",
            ));
        }
        let creds = Credentials::new(
            credentials.access_key_id().expose(),
            credentials.secret_access_key().expose(),
            None,
            None,
            "open-compute-artifacts",
        );
        let timeout = TimeoutConfig::builder()
            .connect_timeout(Duration::from_millis(config.connect_timeout_ms))
            .operation_timeout(Duration::from_millis(config.request_timeout_ms))
            .build();
        let retry = RetryConfig::standard()
            .with_max_attempts(config.max_retries.saturating_add(1).min(8))
            .with_initial_backoff(Duration::from_millis(config.retry_backoff_ms));
        let conf = aws_sdk_s3::config::Builder::new()
            .behavior_version(BehaviorVersion::latest())
            .region(Region::new(config.region.clone()))
            .endpoint_url(&config.endpoint)
            .force_path_style(config.force_path_style)
            .credentials_provider(creds)
            .timeout_config(timeout)
            .retry_config(retry)
            .http_client(build_verified_http_client())
            .request_checksum_calculation(RequestChecksumCalculation::WhenRequired)
            .response_checksum_validation(ResponseChecksumValidation::WhenRequired)
            .build();
        let authority_sha256 = authority_sha256(config);
        Ok(Self {
            inner: Client::from_conf(conf),
            bucket: config.bucket.clone(),
            prefix: config.prefix.clone(),
            r2_prefix: config.r2_prefix.clone(),
            authority_sha256,
            max_artifact_bytes,
        })
    }

    pub(crate) fn prefix(&self) -> &str {
        &self.prefix
    }

    pub(crate) fn r2_prefix(&self) -> &str {
        &self.r2_prefix
    }

    pub(crate) const fn authority_sha256(&self) -> [u8; 32] {
        self.authority_sha256
    }

    pub(crate) fn max_object_bytes(&self) -> u64 {
        self.max_artifact_bytes
    }

    pub(crate) async fn put(
        &self,
        key: &ObjectKey,
        source: ObjectSource,
        mut options: PutOptions,
    ) -> Result<ObjectMetadata, BackendError> {
        if source.length() > self.max_artifact_bytes {
            return Err(BackendError::Capacity);
        }
        let length = source.length();
        let content_md5 = source_content_md5(&source).await?;
        let body = source_stream(source).await?;
        options.metadata.size = length;
        let mut request = self
            .inner
            .put_object()
            .bucket(&self.bucket)
            .key(key.as_str())
            .body(body)
            .content_length(i64::try_from(length).map_err(|_| BackendError::Capacity)?)
            .content_md5(content_md5)
            .set_metadata(Some(options.metadata.user.clone().into_iter().collect()))
            .set_content_type(options.metadata.http.content_type.clone())
            .set_content_language(options.metadata.http.content_language.clone())
            .set_content_disposition(options.metadata.http.content_disposition.clone())
            .set_content_encoding(options.metadata.http.content_encoding.clone())
            .set_cache_control(options.metadata.http.cache_control.clone())
            .set_expires(options.metadata.http.cache_expiry.map(millis_datetime))
            .storage_class(storage_class(options.metadata.storage_class));
        request = match options.mode {
            PutMode::CreateOnly => request.if_none_match("*"),
            PutMode::Replace => request,
            PutMode::IfMatch(etag) => request.if_match(quote_etag(&etag)),
        };
        if let Some(customer) = &options.customer_key {
            request = request
                .sse_customer_algorithm("AES256")
                .sse_customer_key(customer_base64(customer))
                .sse_customer_key_md5(customer_md5(customer));
        }
        request
            .send()
            .await
            .map_err(|error| map_sdk_error(&error, true, options.customer_key.is_some(), false))?;
        self.head(
            key,
            HeadOptions {
                customer_key: options.customer_key,
            },
        )
        .await
    }

    pub(crate) async fn head(
        &self,
        key: &ObjectKey,
        options: HeadOptions,
    ) -> Result<ObjectMetadata, BackendError> {
        let mut request = self
            .inner
            .head_object()
            .bucket(&self.bucket)
            .key(key.as_str());
        if let Some(customer) = &options.customer_key {
            request = request
                .sse_customer_algorithm("AES256")
                .sse_customer_key(customer_base64(customer))
                .sse_customer_key_md5(customer_md5(customer));
        }
        let output = request
            .send()
            .await
            .map_err(|error| map_sdk_error(&error, false, options.customer_key.is_some(), true))?;
        metadata_from_s3(
            output.content_length(),
            output.e_tag(),
            output.last_modified(),
            output.metadata(),
            output.content_type(),
            output.content_language(),
            output.content_disposition(),
            output.content_encoding(),
            output.cache_control(),
            output.expires_string(),
            output.storage_class().map(StorageClass::as_str),
        )
    }

    pub(crate) async fn get(
        &self,
        key: &ObjectKey,
        options: GetOptions,
    ) -> Result<ObjectGet, BackendError> {
        let mut request = self
            .inner
            .get_object()
            .bucket(&self.bucket)
            .key(key.as_str());
        if let Some(range) = options.range {
            if range.end < range.start {
                return Err(BackendError::InvalidRange);
            }
            request = request.range(format!("bytes={}-{}", range.start, range.end));
        }
        if let Some(etag) = &options.if_match {
            request = request.if_match(quote_etag(etag));
        }
        if let Some(customer) = &options.customer_key {
            request = request
                .sse_customer_algorithm("AES256")
                .sse_customer_key(customer_base64(customer))
                .sse_customer_key_md5(customer_md5(customer));
        }
        let output = request
            .send()
            .await
            .map_err(|error| map_sdk_error(&error, true, options.customer_key.is_some(), true))?;
        let content_range = output.content_range().and_then(parse_content_range);
        let returned_range = content_range.map(|(range, _)| range).or(options.range);
        let full_size = content_range
            .and_then(|(_, total)| i64::try_from(total).ok())
            .or(output.content_length());
        let metadata = metadata_from_s3(
            full_size,
            output.e_tag(),
            output.last_modified(),
            output.metadata(),
            output.content_type(),
            output.content_language(),
            output.content_disposition(),
            output.content_encoding(),
            output.cache_control(),
            output.expires_string(),
            output.storage_class().map(StorageClass::as_str),
        )?;
        Ok(ObjectGet {
            metadata,
            range: returned_range,
            body: ObjectBody::from_s3(S3ObjectBody::new(output.body)),
        })
    }

    pub(crate) async fn delete(&self, key: &ObjectKey) -> Result<(), BackendError> {
        self.inner
            .delete_object()
            .bucket(&self.bucket)
            .key(key.as_str())
            .send()
            .await
            .map_err(|error| map_sdk_error(&error, false, false, false))?;
        Ok(())
    }

    pub(crate) async fn delete_many(&self, keys: &[ObjectKey]) -> Result<bool, BackendError> {
        if keys.is_empty() {
            return Ok(true);
        }
        let identifiers = keys
            .iter()
            .map(|key| {
                ObjectIdentifier::builder()
                    .key(key.as_str())
                    .build()
                    .map_err(|_| BackendError::InvalidKey)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let delete = Delete::builder()
            .set_objects(Some(identifiers))
            .quiet(true)
            .build()
            .map_err(|_| BackendError::InvalidKey)?;
        match self
            .inner
            .delete_objects()
            .bucket(&self.bucket)
            .delete(delete)
            .send()
            .await
        {
            Ok(output) if output.errors().is_empty() => Ok(true),
            Err(error) if matches!(sdk_status(&error), Some(405 | 501)) => {
                for key in keys {
                    self.delete(key).await?;
                }
                Ok(false)
            }
            Ok(_) | Err(_) => Err(BackendError::Unavailable),
        }
    }

    pub(crate) async fn list(
        &self,
        prefix: &str,
        limit: u16,
        cursor: Option<&str>,
    ) -> Result<ListPage, BackendError> {
        if limit == 0 || prefix.len() > OBJECT_KEY_MAX_BYTES {
            return Err(BackendError::InvalidKey);
        }
        let output = self
            .inner
            .list_objects_v2()
            .bucket(&self.bucket)
            .prefix(prefix)
            .max_keys(i32::from(limit))
            .set_continuation_token(cursor.map(str::to_owned))
            .send()
            .await
            .map_err(|error| map_sdk_error(&error, false, false, false))?;
        let mut objects = Vec::with_capacity(output.contents().len());
        for object in output.contents() {
            let key = ObjectKey::new(object.key().ok_or(BackendError::Corrupt)?.to_owned())?;
            let size = u64::try_from(object.size().ok_or(BackendError::Corrupt)?)
                .map_err(|_| BackendError::Corrupt)?;
            let etag = unquote_etag(object.e_tag().ok_or(BackendError::Corrupt)?)?;
            objects.push(ListedObject {
                key,
                metadata: ObjectMetadata {
                    size,
                    etag,
                    last_modified_ms: object
                        .last_modified()
                        .and_then(|value| value.to_millis().ok())
                        .unwrap_or(0),
                    storage_class: object_storage_class(
                        object.storage_class().map(S3ObjectStorageClass::as_str),
                    )?,
                    ..ObjectMetadata::default()
                },
            });
        }
        objects.sort_by(|left, right| left.key.cmp(&right.key));
        Ok(ListPage {
            objects,
            next_cursor: output.next_continuation_token().map(str::to_owned),
        })
    }

    pub(crate) async fn create_multipart(
        &self,
        key: &ObjectKey,
        metadata: ObjectMetadata,
        customer_key: Option<CustomerKey>,
    ) -> Result<String, BackendError> {
        let mut request = self
            .inner
            .create_multipart_upload()
            .bucket(&self.bucket)
            .key(key.as_str())
            .set_metadata(Some(metadata.user.into_iter().collect()))
            .set_content_type(metadata.http.content_type)
            .set_content_language(metadata.http.content_language)
            .set_content_disposition(metadata.http.content_disposition)
            .set_content_encoding(metadata.http.content_encoding)
            .set_cache_control(metadata.http.cache_control)
            .set_expires(metadata.http.cache_expiry.map(millis_datetime))
            .storage_class(storage_class(metadata.storage_class));
        if let Some(customer) = &customer_key {
            request = request
                .sse_customer_algorithm("AES256")
                .sse_customer_key(customer_base64(customer))
                .sse_customer_key_md5(customer_md5(customer));
        }
        let output = request
            .send()
            .await
            .map_err(|error| map_sdk_error(&error, false, customer_key.is_some(), false))?;
        output
            .upload_id()
            .map(str::to_owned)
            .ok_or(BackendError::Unavailable)
    }

    pub(crate) async fn upload_part(
        &self,
        key: &ObjectKey,
        upload_id: &str,
        part_number: i32,
        source: ObjectSource,
        customer_key: Option<CustomerKey>,
    ) -> Result<UploadedPart, BackendError> {
        if part_number <= 0 || upload_id.is_empty() || source.length() > self.max_artifact_bytes {
            return Err(BackendError::MultipartInvalid);
        }
        let length = source.length();
        let content_md5 = source_content_md5(&source).await?;
        let body = source_stream(source).await?;
        let mut request = self
            .inner
            .upload_part()
            .bucket(&self.bucket)
            .key(key.as_str())
            .upload_id(upload_id)
            .part_number(part_number)
            .body(body)
            .content_length(i64::try_from(length).map_err(|_| BackendError::Capacity)?)
            .content_md5(content_md5);
        if let Some(customer) = &customer_key {
            request = request
                .sse_customer_algorithm("AES256")
                .sse_customer_key(customer_base64(customer))
                .sse_customer_key_md5(customer_md5(customer));
        }
        let output = request
            .send()
            .await
            .map_err(|error| map_sdk_error(&error, false, customer_key.is_some(), false))?;
        Ok(UploadedPart {
            part_number,
            etag: unquote_etag(output.e_tag().ok_or(BackendError::Unavailable)?)?,
        })
    }

    pub(crate) async fn list_multipart(
        &self,
        key: &ObjectKey,
    ) -> Result<Vec<String>, BackendError> {
        let mut key_marker = None;
        let mut upload_id_marker = None;
        let mut ids = Vec::new();
        for _ in 0..100 {
            let output = self
                .inner
                .list_multipart_uploads()
                .bucket(&self.bucket)
                .prefix(key.as_str())
                .max_uploads(1000)
                .set_key_marker(key_marker.clone())
                .set_upload_id_marker(upload_id_marker.clone())
                .send()
                .await
                .map_err(|error| map_sdk_error(&error, false, false, false))?;
            for upload in output.uploads() {
                if upload.key() == Some(key.as_str()) {
                    ids.push(
                        upload
                            .upload_id()
                            .map(str::to_owned)
                            .ok_or(BackendError::Corrupt)?,
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
                return Err(BackendError::Unavailable);
            }
            key_marker = next_key;
            upload_id_marker = next_upload;
        }
        Err(BackendError::Unavailable)
    }

    pub(crate) async fn complete_multipart(
        &self,
        key: &ObjectKey,
        upload_id: &str,
        parts: &[UploadedPart],
        customer_key: Option<CustomerKey>,
    ) -> Result<ObjectMetadata, BackendError> {
        if upload_id.is_empty() || parts.is_empty() {
            return Err(BackendError::MultipartInvalid);
        }
        let completed = parts
            .iter()
            .map(|part| {
                CompletedPart::builder()
                    .e_tag(quote_etag(&part.etag))
                    .part_number(part.part_number)
                    .build()
            })
            .collect::<Vec<_>>();
        let upload = CompletedMultipartUpload::builder()
            .set_parts(Some(completed))
            .build();
        let mut request = self
            .inner
            .complete_multipart_upload()
            .bucket(&self.bucket)
            .key(key.as_str())
            .upload_id(upload_id)
            .multipart_upload(upload);
        if let Some(customer) = &customer_key {
            request = request
                .sse_customer_algorithm("AES256")
                .sse_customer_key(customer_base64(customer))
                .sse_customer_key_md5(customer_md5(customer));
        }
        request
            .send()
            .await
            .map_err(|error| map_sdk_error(&error, false, customer_key.is_some(), false))?;
        self.head(key, HeadOptions { customer_key }).await
    }

    pub(crate) async fn abort_multipart(
        &self,
        key: &ObjectKey,
        upload_id: &str,
    ) -> Result<(), BackendError> {
        match self
            .inner
            .abort_multipart_upload()
            .bucket(&self.bucket)
            .key(key.as_str())
            .upload_id(upload_id)
            .send()
            .await
        {
            Ok(_) => Ok(()),
            Err(error) if sdk_status(&error) == Some(404) => Ok(()),
            Err(error) => Err(map_sdk_error(&error, false, false, false)),
        }
    }
}

async fn source_stream(source: ObjectSource) -> Result<ByteStream, BackendError> {
    match source {
        ObjectSource::Bytes(bytes) => Ok(ByteStream::from(bytes)),
        ObjectSource::File { mut file, length } => {
            let metadata = file.metadata().map_err(|_| BackendError::Unavailable)?;
            if !metadata.file_type().is_file() || metadata.len() != length {
                return Err(BackendError::Corrupt);
            }
            file.seek(SeekFrom::Start(0))
                .map_err(|_| BackendError::Unavailable)?;
            ByteStream::read_from()
                .file(tokio::fs::File::from_std(file))
                .length(aws_smithy_types::byte_stream::Length::Exact(length))
                .buffer_size(64 * 1024)
                .build()
                .await
                .map_err(|_| BackendError::Unavailable)
        }
    }
}

async fn source_content_md5(source: &ObjectSource) -> Result<String, BackendError> {
    match source {
        ObjectSource::Bytes(bytes) => {
            Ok(base64::engine::general_purpose::STANDARD.encode(Md5::digest(bytes.as_ref())))
        }
        ObjectSource::File { file, length } => {
            let file = file.try_clone().map_err(|_| BackendError::Unavailable)?;
            let length = *length;
            tokio::task::spawn_blocking(move || content_md5_at(&file, length))
                .await
                .map_err(|_| BackendError::Unavailable)?
        }
    }
}

fn content_md5_at(file: &std::fs::File, length: u64) -> Result<String, BackendError> {
    let mut digest = Md5::new();
    let mut offset = 0_u64;
    let mut buffer = vec![0_u8; 64 * 1024];
    while offset < length {
        let remaining = usize::try_from((length - offset).min(buffer.len() as u64))
            .map_err(|_| BackendError::Capacity)?;
        let count = file
            .read_at(&mut buffer[..remaining], offset)
            .map_err(|_| BackendError::Unavailable)?;
        if count == 0 {
            return Err(BackendError::Corrupt);
        }
        digest.update(&buffer[..count]);
        offset = offset
            .checked_add(count as u64)
            .ok_or(BackendError::Capacity)?;
    }
    Ok(base64::engine::general_purpose::STANDARD.encode(digest.finalize()))
}

#[allow(clippy::too_many_arguments)]
fn metadata_from_s3(
    size: Option<i64>,
    etag: Option<&str>,
    modified: Option<&DateTime>,
    user: Option<&std::collections::HashMap<String, String>>,
    content_type: Option<&str>,
    content_language: Option<&str>,
    content_disposition: Option<&str>,
    content_encoding: Option<&str>,
    cache_control: Option<&str>,
    expires: Option<&str>,
    storage_class: Option<&str>,
) -> Result<ObjectMetadata, BackendError> {
    Ok(ObjectMetadata {
        size: u64::try_from(size.ok_or(BackendError::Corrupt)?)
            .map_err(|_| BackendError::Corrupt)?,
        etag: unquote_etag(etag.ok_or(BackendError::Corrupt)?)?,
        last_modified_ms: modified
            .and_then(|value| value.to_millis().ok())
            .unwrap_or(0),
        user: user
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .collect::<BTreeMap<_, _>>(),
        http: crate::backend::ObjectHttpMetadata {
            content_type: content_type.map(str::to_owned),
            content_language: content_language.map(str::to_owned),
            content_disposition: content_disposition.map(str::to_owned),
            content_encoding: content_encoding.map(str::to_owned),
            cache_control: cache_control.map(str::to_owned),
            cache_expiry: expires.and_then(http_date_millis),
        },
        storage_class: object_storage_class(storage_class)?,
        ssec_key_md5: None,
    })
}

const fn storage_class(value: ObjectStorageClass) -> StorageClass {
    match value {
        ObjectStorageClass::Standard => StorageClass::Standard,
        ObjectStorageClass::InfrequentAccess => StorageClass::StandardIa,
    }
}

fn object_storage_class(value: Option<&str>) -> Result<ObjectStorageClass, BackendError> {
    match value.unwrap_or("STANDARD") {
        "STANDARD" => Ok(ObjectStorageClass::Standard),
        "STANDARD_IA" => Ok(ObjectStorageClass::InfrequentAccess),
        _ => Err(BackendError::Corrupt),
    }
}

fn quote_etag(value: &str) -> String {
    format!("\"{}\"", value.trim_matches('"'))
}

fn unquote_etag(value: &str) -> Result<String, BackendError> {
    let value = value.trim_matches('"');
    if value.is_empty()
        || value
            .bytes()
            .any(|byte| byte.is_ascii_control() || byte == b'"')
    {
        return Err(BackendError::Corrupt);
    }
    Ok(value.to_owned())
}

fn customer_base64(key: &CustomerKey) -> String {
    base64::engine::general_purpose::STANDARD.encode(key.bytes())
}

fn customer_md5(key: &CustomerKey) -> String {
    base64::engine::general_purpose::STANDARD.encode(Md5::digest(key.bytes()))
}

pub(crate) fn millis_datetime(value: i64) -> DateTime {
    let seconds = value.div_euclid(1000);
    let nanos = u32::try_from(value.rem_euclid(1000)).unwrap_or(0) * 1_000_000;
    DateTime::from_secs_and_nanos(seconds, nanos)
}

pub(crate) fn http_date_millis(value: &str) -> Option<i64> {
    use aws_smithy_types::date_time::Format;
    DateTime::from_str(value, Format::HttpDate)
        .ok()
        .and_then(|date| date.to_millis().ok())
}

pub(crate) fn parse_content_range(value: &str) -> Option<(ObjectRange, u64)> {
    let value = value.strip_prefix("bytes ")?;
    let (range, total) = value.split_once('/')?;
    let (start, end) = range.split_once('-')?;
    let start = start.parse().ok()?;
    let end = end.parse().ok()?;
    let total = total.parse().ok()?;
    if start > end || end >= total {
        return None;
    }
    Some((ObjectRange { start, end }, total))
}

fn sdk_status<E>(error: &SdkError<E, HttpResponse>) -> Option<u16> {
    match error {
        SdkError::ServiceError(service) => Some(service.raw().status().as_u16()),
        SdkError::ResponseError(response) => Some(response.raw().status().as_u16()),
        _ => None,
    }
}

fn map_sdk_error<E>(
    error: &SdkError<E, HttpResponse>,
    condition_or_range: bool,
    customer_key_present: bool,
    customer_key_context: bool,
) -> BackendError {
    match sdk_status(error) {
        Some(404) => BackendError::NotFound,
        Some(409 | 412) if condition_or_range => BackendError::PreconditionFailed,
        Some(416) if condition_or_range => BackendError::InvalidRange,
        Some(400) if customer_key_present || customer_key_context => {
            BackendError::CustomerKeyInvalid
        }
        Some(403) if customer_key_present => BackendError::CustomerKeyInvalid,
        Some(507) => BackendError::Capacity,
        _ => BackendError::Unavailable,
    }
}

fn authority_sha256(config: &S3Config) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(b"open-compute/object-authority/s3/v1");
    for value in [
        config.endpoint.as_str(),
        config.region.as_str(),
        config.bucket.as_str(),
        config.prefix.as_str(),
        config.r2_prefix.as_str(),
    ] {
        digest.update((value.len() as u64).to_be_bytes());
        digest.update(value.as_bytes());
    }
    digest.update([u8::from(config.force_path_style)]);
    digest.finalize().into()
}

fn build_verified_http_client() -> aws_smithy_runtime_api::client::http::SharedHttpClient {
    let mut trust = TrustStore::empty();
    for cert in webpki_root_certs::TLS_SERVER_ROOT_CERTS {
        trust = trust.with_pem_certificate(der_to_pem(cert.as_ref()));
    }
    let tls = TlsContext::builder()
        .with_trust_store(trust)
        .build()
        .unwrap_or_else(|_| {
            TlsContext::builder()
                .with_trust_store(TrustStore::empty())
                .build()
                .unwrap_or_else(|_| {
                    TlsContext::builder()
                        .build()
                        .unwrap_or_else(|_| unreachable!("tls context builder"))
                })
        });
    HttpBuilder::new()
        .tls_provider(TlsProvider::Rustls(CryptoMode::AwsLc))
        .tls_context(tls)
        .build_https()
}

fn der_to_pem(der: &[u8]) -> Vec<u8> {
    use base64::Engine as _;
    let encoded = base64::engine::general_purpose::STANDARD.encode(der);
    let mut pem = b"-----BEGIN CERTIFICATE-----\n".to_vec();
    for chunk in encoded.as_bytes().chunks(64) {
        pem.extend_from_slice(chunk);
        pem.push(b'\n');
    }
    pem.extend_from_slice(b"-----END CERTIFICATE-----\n");
    pem
}
