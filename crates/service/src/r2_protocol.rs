//! Private R2 wire types, validation, and sanitized HTTP responses.

use crate::metrics::R2Operation;
use axum::body::{Body, to_bytes};
use axum::http::{HeaderName, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use base64::Engine as _;
use bytes::Bytes;
use open_compute_artifacts::{
    R2_MAX_LIST_LIMIT, R2Condition, R2HttpMetadata, R2PutOptions, R2Range,
};
use open_compute_core::{BindingId, BindingKind, ErrorCode, PlatformError, ResourceId};
use open_compute_storage::{AuthorizedBinding, PlatformStorage};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use std::collections::BTreeMap;
use std::future::Future;
use std::str::FromStr;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub(crate) const JSON_CONTENT_TYPE: &str = "application/vnd.open-compute.r2.v1+json";
pub(crate) const FRAME_CONTENT_TYPE: &str = "application/vnd.open-compute.r2.v1+frame";
pub(crate) const MAX_METADATA_BYTES: usize = 16 * 1024;
pub(crate) const MAX_DELETE_BODY_BYTES: usize = 1024 * 1024 + MAX_METADATA_BYTES;
pub(crate) const ERROR_HEADER: &str = "x-open-compute-error-code";

#[derive(Clone, Copy, Debug)]
pub(crate) enum Operation {
    Head,
    Get,
    Put,
    Delete,
    List,
}

impl Operation {
    pub(crate) const fn write(self) -> bool {
        matches!(self, Self::Put | Self::Delete)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct KeyRequest {
    pub(crate) key: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct GetRequest {
    pub(crate) key: String,
    #[serde(default)]
    pub(crate) options: GetOptions,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct GetOptions {
    pub(crate) range: Option<R2Range>,
    pub(crate) only_if: Option<R2Condition>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PutHeader {
    pub(crate) key: String,
    #[serde(default)]
    pub(crate) options: PutWireOptions,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct PutWireOptions {
    #[serde(default)]
    only_if: Option<R2Condition>,
    #[serde(default)]
    http_metadata: R2HttpMetadata,
    #[serde(default)]
    custom_metadata: BTreeMap<String, String>,
    md5: Option<serde_json::Value>,
    storage_class: Option<String>,
}

impl PutWireOptions {
    pub(crate) fn validate(&self) -> Result<(), PlatformError> {
        if self
            .storage_class
            .as_deref()
            .is_some_and(|value| value != "Standard")
        {
            return Err(PlatformError::new(
                ErrorCode::R2UnsupportedFeature,
                "R2 storage class is unsupported",
            ));
        }
        if self.only_if.as_ref().is_some_and(|condition| {
            condition.uploaded_before.is_some() || condition.uploaded_after.is_some()
        }) {
            return Err(PlatformError::new(
                ErrorCode::R2UnsupportedCondition,
                "R2 conditional PUT supports only ETags",
            ));
        }
        Ok(())
    }
}

impl TryFrom<PutWireOptions> for R2PutOptions {
    type Error = PlatformError;

    fn try_from(value: PutWireOptions) -> Result<Self, Self::Error> {
        value.validate()?;
        Ok(Self {
            http_metadata: value.http_metadata,
            custom_metadata: value.custom_metadata,
            only_if: value.only_if,
            expected_md5: value.md5.as_ref().map(parse_md5).transpose()?,
        })
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DeleteRequest {
    pub(crate) keys: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ListRequest {
    #[serde(default)]
    pub(crate) prefix: String,
    pub(crate) delimiter: Option<String>,
    pub(crate) cursor: Option<String>,
    #[serde(default = "default_limit")]
    pub(crate) limit: u16,
    #[serde(default)]
    pub(crate) include: Vec<String>,
}

impl ListRequest {
    pub(crate) fn validate(&self) -> Result<(), PlatformError> {
        if self.limit == 0
            || self.limit > R2_MAX_LIST_LIMIT
            || self.delimiter.as_deref().is_some_and(str::is_empty)
            || self.include.len() > 2
        {
            return Err(invalid_options());
        }
        Ok(())
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ListResponse {
    pub(crate) objects: Vec<serde_json::Value>,
    pub(crate) truncated: bool,
    pub(crate) cursor: Option<String>,
    pub(crate) delimited_prefixes: Vec<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct CursorPayload {
    pub(crate) v: u8,
    pub(crate) resource_id: ResourceId,
    pub(crate) generation: u64,
    pub(crate) prefix_sha256: String,
    pub(crate) delimiter_sha256: String,
    pub(crate) include_mask: u8,
    pub(crate) provider_token: String,
    pub(crate) expires_at_ms: u64,
}

pub(crate) fn minimal_list_metadata(
    object: &open_compute_artifacts::R2ListedObject,
) -> serde_json::Value {
    serde_json::json!({ "key": object.key, "size": object.size, "etag": object.etag, "httpEtag": format!("\"{}\"", object.etag), "uploaded": object.uploaded, "httpMetadata": {}, "customMetadata": {}, "storageClass": "Standard" })
}

pub(crate) fn include_mask(include: &[String]) -> Result<u8, PlatformError> {
    let mut mask = 0_u8;
    for item in include {
        mask |= match item.as_str() {
            "httpMetadata" => 1,
            "customMetadata" => 2,
            _ => return Err(invalid_options()),
        };
    }
    Ok(mask)
}

fn parse_md5(value: &serde_json::Value) -> Result<[u8; 16], PlatformError> {
    let bytes = if let Some(text) = value.as_str() {
        if text.len() == 32 {
            hex::decode(text).map_err(|_| invalid_options())?
        } else {
            base64::engine::general_purpose::STANDARD
                .decode(text)
                .map_err(|_| invalid_options())?
        }
    } else if let Some(values) = value.as_array() {
        values
            .iter()
            .map(|value| {
                value
                    .as_u64()
                    .and_then(|value| u8::try_from(value).ok())
                    .ok_or_else(invalid_options)
            })
            .collect::<Result<Vec<_>, _>>()?
    } else {
        return Err(invalid_options());
    };
    bytes.try_into().map_err(|_| invalid_options())
}

pub(crate) fn parse_path(path: &str) -> Result<(BindingId, Operation), PlatformError> {
    let rest = path
        .strip_prefix("/internal/bindings/v1/r2/")
        .ok_or_else(protocol_error)?;
    let (id, operation) = rest.split_once('/').ok_or_else(protocol_error)?;
    if operation.contains('/') {
        return Err(protocol_error());
    }
    let operation = match operation {
        "head" => Operation::Head,
        "get" => Operation::Get,
        "put" => Operation::Put,
        "delete" => Operation::Delete,
        "list" => Operation::List,
        _ => return Err(protocol_error()),
    };
    Ok((
        BindingId::from_str(id).map_err(|_| protocol_error())?,
        operation,
    ))
}

pub(crate) fn validate_binding(
    binding: &AuthorizedBinding,
    operation: Operation,
) -> Result<(), PlatformError> {
    if binding.binding.kind != BindingKind::R2Bucket || binding.binding.capability_version != 1 {
        return Err(PlatformError::new(
            ErrorCode::BindingCapabilityUnsupported,
            "R2 binding capability is unsupported",
        ));
    }
    let allowed = if operation.write() {
        binding.binding.permissions.write
    } else {
        binding.binding.permissions.read
    };
    if !allowed {
        return Err(PlatformError::new(
            ErrorCode::BindingPermissionDenied,
            "R2 binding permission denied",
        ));
    }
    Ok(())
}

pub(crate) fn content_type_matches(headers: &axum::http::HeaderMap, operation: Operation) -> bool {
    let expected = if matches!(operation, Operation::Get | Operation::Put) {
        FRAME_CONTENT_TYPE
    } else {
        JSON_CONTENT_TYPE
    };
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .is_some_and(|value| value.trim() == expected)
}

pub(crate) fn parse_header<T: FromStr>(
    headers: &axum::http::HeaderMap,
    name: &str,
) -> Result<T, PlatformError> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| T::from_str(value).ok())
        .ok_or_else(protocol_error)
}

pub(crate) fn parse_digest(headers: &axum::http::HeaderMap) -> Result<[u8; 32], PlatformError> {
    let value = headers
        .get("x-open-compute-descriptor-sha256")
        .and_then(|value| value.to_str().ok())
        .ok_or_else(protocol_error)?;
    hex::decode(value)
        .map_err(|_| protocol_error())?
        .try_into()
        .map_err(|_| protocol_error())
}

pub(crate) fn parse_request_id(headers: &axum::http::HeaderMap) -> Result<String, PlatformError> {
    let value = headers
        .get("x-open-compute-request-id")
        .and_then(|value| value.to_str().ok())
        .ok_or_else(protocol_error)?;
    let id = uuid::Uuid::parse_str(value).map_err(|_| protocol_error())?;
    if id.hyphenated().to_string() != value {
        return Err(protocol_error());
    }
    Ok(value.to_owned())
}

pub(crate) async fn bounded_json(body: Body, limit: usize) -> Result<Bytes, PlatformError> {
    to_bytes(body, limit).await.map_err(|_| protocol_error())
}

pub(crate) fn parse_json<T: for<'de> Deserialize<'de>>(bytes: &[u8]) -> Result<T, PlatformError> {
    serde_json::from_slice(bytes).map_err(|_| protocol_error())
}

pub(crate) async fn timeout_result<T>(
    duration: Duration,
    future: impl Future<Output = Result<T, PlatformError>>,
) -> Result<T, PlatformError> {
    tokio::time::timeout(duration, future).await.map_err(|_| {
        PlatformError::new(ErrorCode::R2ProviderUnavailable, "R2 operation timed out")
    })?
}

pub(crate) async fn mutation_timeout_result<T>(
    duration: Duration,
    future: impl Future<Output = Result<T, PlatformError>>,
) -> Result<T, PlatformError> {
    tokio::time::timeout(duration, future).await.map_err(|_| {
        PlatformError::new(
            ErrorCode::R2ResultUnknown,
            "R2 mutation result is unknown after timeout",
        )
    })?
}

pub(crate) fn json_response(value: impl Serialize) -> Response {
    (
        [(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        )],
        axum::Json(value),
    )
        .into_response()
}

pub(crate) fn no_content() -> Response {
    StatusCode::NO_CONTENT.into_response()
}

pub(crate) fn digest_text(value: &str) -> String {
    hex::encode(Sha256::digest(value.as_bytes()))
}

pub(crate) fn unix_ms() -> Result<u64, PlatformError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| cursor_invalid())?
        .as_millis()
        .try_into()
        .map_err(|_| cursor_invalid())
}

const fn default_limit() -> u16 {
    R2_MAX_LIST_LIMIT
}

pub(crate) fn ensure_storage_headroom(
    storage: &PlatformStorage,
    additional: u64,
) -> Result<(), PlatformError> {
    let stat = rustix::fs::statvfs(storage.data_dir().root()).map_err(|_| overloaded())?;
    let available = stat.f_bavail.saturating_mul(stat.f_frsize);
    let required = storage
        .free_space_hard_bytes()
        .checked_add(additional)
        .ok_or_else(overloaded)?;
    if available < required {
        return Err(overloaded());
    }
    Ok(())
}

pub(crate) fn error_response(error: &PlatformError) -> Response {
    let status = match error.code() {
        ErrorCode::BindingNotFound | ErrorCode::ResourceNotFound => StatusCode::NOT_FOUND,
        ErrorCode::BindingPermissionDenied => StatusCode::FORBIDDEN,
        ErrorCode::R2ObjectTooLarge | ErrorCode::R2KeyTooLarge | ErrorCode::R2MetadataTooLarge => {
            StatusCode::PAYLOAD_TOO_LARGE
        }
        ErrorCode::ResourceNotReady | ErrorCode::R2BucketNotEmpty => StatusCode::CONFLICT,
        ErrorCode::ResourceUnavailable
        | ErrorCode::R2Overloaded
        | ErrorCode::R2ProviderUnavailable
        | ErrorCode::R2ResultUnknown => StatusCode::SERVICE_UNAVAILABLE,
        ErrorCode::BindingTypeMismatch
        | ErrorCode::BindingCapabilityUnsupported
        | ErrorCode::R2ObjectMetadataInvalid
        | ErrorCode::R2PrefixCollision
        | ErrorCode::ResourceInvariantViolation => StatusCode::UNPROCESSABLE_ENTITY,
        ErrorCode::BindingProtocolError
        | ErrorCode::R2KeyInvalid
        | ErrorCode::R2InvalidOptions
        | ErrorCode::R2UnsupportedCondition
        | ErrorCode::R2UnsupportedFeature
        | ErrorCode::R2CursorInvalid
        | ErrorCode::R2PreconditionFailed => StatusCode::BAD_REQUEST,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    };
    let retryable = matches!(
        error.code(),
        ErrorCode::ResourceUnavailable
            | ErrorCode::R2Overloaded
            | ErrorCode::R2ProviderUnavailable
            | ErrorCode::R2ResultUnknown
    );
    let mut response = (status, axum::Json(serde_json::json!({"ok": false, "error": {"code": error.code().as_str(), "retryable": retryable, "resultUnknown": error.code() == ErrorCode::R2ResultUnknown}}))).into_response();
    if let Ok(value) = HeaderValue::from_str(error.code().as_str()) {
        response
            .headers_mut()
            .insert(HeaderName::from_static(ERROR_HEADER), value);
    }
    response
}

pub(crate) fn protocol_error() -> PlatformError {
    PlatformError::new(
        ErrorCode::BindingProtocolError,
        "R2 private protocol is invalid",
    )
}

fn invalid_options() -> PlatformError {
    PlatformError::new(ErrorCode::R2InvalidOptions, "R2 options are invalid")
}

pub(crate) fn metric_operation(path: &str) -> Option<R2Operation> {
    match path.rsplit('/').next()? {
        "head" => Some(R2Operation::Head),
        "get" => Some(R2Operation::Get),
        "put" => Some(R2Operation::Put),
        "delete" => Some(R2Operation::Delete),
        "list" => Some(R2Operation::List),
        _ => None,
    }
}

pub(crate) fn metadata_too_large() -> PlatformError {
    PlatformError::new(
        ErrorCode::R2MetadataTooLarge,
        "R2 metadata frame exceeds its fixed budget",
    )
}

pub(crate) fn metadata_invalid() -> PlatformError {
    PlatformError::new(
        ErrorCode::R2ObjectMetadataInvalid,
        "R2 object metadata is unavailable",
    )
}

pub(crate) fn cursor_invalid() -> PlatformError {
    PlatformError::new(ErrorCode::R2CursorInvalid, "R2 list cursor is invalid")
}

pub(crate) fn object_too_large() -> PlatformError {
    PlatformError::new(
        ErrorCode::R2ObjectTooLarge,
        "R2 object exceeds the bucket limit",
    )
}

pub(crate) fn overloaded() -> PlatformError {
    PlatformError::new(
        ErrorCode::R2Overloaded,
        "R2 host capacity is temporarily saturated",
    )
}

#[cfg(test)]
#[path = "r2_protocol_tests.rs"]
mod tests;
