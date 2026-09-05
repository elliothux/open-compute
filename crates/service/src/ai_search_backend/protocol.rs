//! AI Search private wire values, framing, response shaping, and errors.

use super::*;

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ListItems {
    pub(super) page: Option<u64>,
    pub(super) per_page: Option<u32>,
    pub(super) search: Option<String>,
    pub(super) sort_by: Option<String>,
    pub(super) status: Option<String>,
    pub(super) source: Option<String>,
    pub(super) metadata_filter: Option<String>,
    pub(super) item_id: Option<String>,
    pub(super) key: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ItemPayload {
    pub(super) item_id: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct CursorPage {
    pub(super) limit: Option<u32>,
    pub(super) cursor: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ItemLogsPayload {
    pub(super) item_id: String,
    pub(super) params: CursorPage,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct OffsetPage {
    pub(super) limit: Option<u32>,
    pub(super) offset: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ItemChunksPayload {
    pub(super) item_id: String,
    pub(super) params: OffsetPage,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Page {
    pub(super) page: Option<u64>,
    pub(super) per_page: Option<u32>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct CreateJob {
    pub(super) description: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct JobPayload {
    pub(super) job_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct JobLogsPayload {
    pub(super) job_id: String,
    pub(super) params: Page,
}

pub(super) fn item_info_value(item: &AiSearchItemRecord) -> Result<Value, PlatformError> {
    let metadata: Value = serde_json::from_slice(&item.metadata_json).map_err(|_| corrupt())?;
    Ok(json!({
        "id": item.id,
        "key": item.key,
        "status": item.status,
        "next_action": if matches!(item.status.as_str(), "queued" | "running" | "outdated") {
            Some("INDEX")
        } else {
            None
        },
        "checksum": hex::encode(item.object.object_sha256),
        "chunks_count": item.chunks_count,
        "file_size": item.object.object_size,
        "source_id": "builtin",
        "created_at": timestamp(item.created_at_ms)?,
        "last_seen_at": timestamp(item.updated_at_ms)?,
        "metadata": metadata,
    }))
}

pub(super) fn job_info_value(job: &AiSearchJobRecord) -> Result<Value, PlatformError> {
    Ok(json!({
        "id": job.id,
        "source": job.source,
        "description": job.description,
        "last_seen_at": timestamp(job.updated_at_ms)?,
        "started_at": job.started_at_ms.map(timestamp).transpose()?,
        "ended_at": job.ended_at_ms.map(timestamp).transpose()?,
        "end_reason": if matches!(job.state.as_str(), "completed" | "error" | "cancelled" | "outdated") {
            Some(job.state.as_str())
        } else {
            None
        },
    }))
}

pub(super) fn timestamp(value: i64) -> Result<String, PlatformError> {
    jiff::Timestamp::from_millisecond(value)
        .map(|timestamp| timestamp.to_string())
        .map_err(|_| corrupt())
}

pub(super) async fn stage_upload(
    mut body: Body,
    path: std::path::PathBuf,
) -> Result<StagedUpload, PlatformError> {
    let result = async {
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .map_err(|_| unavailable())?;
        let mut file = tokio::fs::File::from_std(file);
        let mut framing = Vec::new();
        let mut header_length = None;
        let mut header = None;
        let mut digest = Sha256::new();
        let mut size = 0_u64;
        while let Some(frame) = body.frame().await {
            let frame = frame.map_err(|_| protocol())?;
            let data = frame.into_data().map_err(|_| protocol())?;
            let mut bytes = data.as_ref();
            if header.is_none() {
                framing.extend_from_slice(bytes);
                if header_length.is_none() && framing.len() >= 4 {
                    let prefix: [u8; 4] = framing[..4].try_into().map_err(|_| protocol())?;
                    let length =
                        usize::try_from(u32::from_be_bytes(prefix)).map_err(|_| limit())?;
                    if length == 0 || length > MAX_FRAME_METADATA_BYTES {
                        return Err(limit());
                    }
                    header_length = Some(length);
                }
                let Some(length) = header_length else {
                    continue;
                };
                let body_start = 4_usize.checked_add(length).ok_or_else(limit)?;
                if framing.len() < body_start {
                    continue;
                }
                let parsed: UploadHeader =
                    serde_json::from_slice(&framing[4..body_start]).map_err(|_| protocol())?;
                if parsed.schema_version != 1 {
                    return Err(protocol());
                }
                bytes = &framing[body_start..];
                header = Some(parsed);
            }
            if !bytes.is_empty() {
                size = size
                    .checked_add(u64::try_from(bytes.len()).map_err(|_| limit())?)
                    .ok_or_else(limit)?;
                if size > MAX_UPLOAD_BYTES as u64 {
                    return Err(limit());
                }
                digest.update(bytes);
                file.write_all(bytes).await.map_err(|_| unavailable())?;
            }
            framing.clear();
        }
        let header = header.ok_or_else(protocol)?;
        if size == 0 {
            return Err(limit());
        }
        file.sync_all().await.map_err(|_| unavailable())?;
        drop(file);
        Ok(StagedUpload {
            header,
            path: path.clone(),
            digest: digest.finalize().into(),
            size,
        })
    }
    .await;
    if result.is_err() {
        let _ = tokio::fs::remove_file(&path).await;
    }
    result
}

pub(super) fn validate_source(
    name: &str,
    content_type: &str,
    size: u64,
) -> Result<(), PlatformError> {
    if name.is_empty()
        || name.len() > 1_024
        || name.chars().any(char::is_control)
        || content_type.is_empty()
        || content_type.len() > 128
        || content_type.chars().any(char::is_control)
        || size == 0
        || size > MAX_UPLOAD_BYTES as u64
    {
        return Err(limit());
    }
    Ok(())
}

pub(super) fn pagination(count: usize, page: u64, per_page: u32, total: usize) -> Value {
    json!({
        "count": count,
        "page": page,
        "per_page": per_page,
        "total_count": total,
    })
}

pub(super) fn metric_operation(operation: &str) -> AiSearchOperation {
    if operation.contains("search") {
        AiSearchOperation::Search
    } else if operation.contains("chatCompletions") {
        AiSearchOperation::Chat
    } else if operation.starts_with("namespace.") {
        AiSearchOperation::Namespace
    } else if operation.starts_with("instance.") {
        AiSearchOperation::Instance
    } else if operation.starts_with("item") {
        AiSearchOperation::Item
    } else {
        AiSearchOperation::Job
    }
}

pub(super) fn page_bounds(
    page: Option<u64>,
    per_page: Option<u32>,
    total: usize,
) -> Result<(u64, u32, usize, usize), PlatformError> {
    let page = page.unwrap_or(1);
    let per_page = per_page.unwrap_or(50);
    if page == 0 || per_page > 100 {
        return Err(protocol());
    }
    if per_page == 0 {
        return Ok((page, per_page, 0, 0));
    }
    let start = page
        .checked_sub(1)
        .and_then(|value| value.checked_mul(u64::from(per_page)))
        .and_then(|value| usize::try_from(value).ok())
        .ok_or_else(limit)?;
    Ok((
        page,
        per_page,
        start.min(total),
        start
            .saturating_add(usize::try_from(per_page).map_err(|_| limit())?)
            .min(total),
    ))
}

pub(super) fn require_namespace(authority: &Authority) -> Result<(), PlatformError> {
    if authority.kind != BindingKind::AiSearchNamespace {
        return Err(unsupported());
    }
    Ok(())
}

pub(super) fn require_permission(authority: &Authority, write: bool) -> Result<(), PlatformError> {
    if write && !authority.write || !write && !authority.read {
        return Err(PlatformError::new(
            ErrorCode::BindingPermissionDenied,
            "AI Search binding permission denied",
        ));
    }
    Ok(())
}

pub(super) fn require_empty_object(value: &Value) -> Result<(), PlatformError> {
    if value.as_object().is_some_and(Map::is_empty) {
        Ok(())
    } else {
        Err(protocol())
    }
}

pub(super) fn parse_header<T: FromStr>(
    headers: &HeaderMap,
    name: &str,
) -> Result<T, PlatformError> {
    header_text(headers, name)?.parse().map_err(|_| protocol())
}

pub(super) fn header_text<'a>(
    headers: &'a HeaderMap,
    name: &str,
) -> Result<&'a str, PlatformError> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty())
        .ok_or_else(protocol)
}

pub(super) fn parse_digest(headers: &HeaderMap, name: &str) -> Result<[u8; 32], PlatformError> {
    let bytes = hex::decode(header_text(headers, name)?).map_err(|_| protocol())?;
    bytes.try_into().map_err(|_| protocol())
}

pub(super) fn content_type_is(headers: &HeaderMap, expected: &str) -> bool {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        == Some(expected)
}

pub(super) fn json_response(value: &impl Serialize) -> Result<Response, PlatformError> {
    let bytes = serde_json::to_vec(value).map_err(|_| protocol())?;
    Ok(([(header::CONTENT_TYPE, "application/json")], bytes).into_response())
}

pub(super) fn error_response(error: &PlatformError) -> Response {
    let status = match error.code() {
        ErrorCode::BindingPermissionDenied => StatusCode::FORBIDDEN,
        ErrorCode::ResourceNotFound => StatusCode::NOT_FOUND,
        ErrorCode::BindingLimitExceeded | ErrorCode::ResourceLimitExceeded => {
            StatusCode::PAYLOAD_TOO_LARGE
        }
        ErrorCode::ResourceUnavailable
        | ErrorCode::ResourceInvariantViolation
        | ErrorCode::ObjectStorageUnavailable
        | ErrorCode::PlatformUnavailable => StatusCode::SERVICE_UNAVAILABLE,
        _ => StatusCode::BAD_REQUEST,
    };
    let mut response = (
        status,
        axum::Json(json!({
            "error": {"code": error.code().as_str(), "message": error.message()}
        })),
    )
        .into_response();
    response.headers_mut().insert(
        "x-open-compute-error-code",
        HeaderValue::from_static(error.code().as_str()),
    );
    response
}

pub(super) fn unix_ms() -> Result<i64, PlatformError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
        .ok_or_else(unavailable)
}

pub(super) fn provider_error(error: crate::ai_provider::AiProviderError) -> PlatformError {
    use crate::ai_provider::AiProviderError;
    match error {
        AiProviderError::InvalidRequest
        | AiProviderError::ContractMismatch
        | AiProviderError::Permanent
        | AiProviderError::MalformedResponse => unsupported(),
        AiProviderError::Unauthorized
        | AiProviderError::RateLimited { .. }
        | AiProviderError::Transient
        | AiProviderError::Timeout => unavailable(),
    }
}

pub(super) fn protocol() -> PlatformError {
    PlatformError::new(
        ErrorCode::BindingProtocolError,
        "AI Search private request is invalid",
    )
}

pub(super) fn limit() -> PlatformError {
    PlatformError::new(
        ErrorCode::BindingLimitExceeded,
        "AI Search request exceeds a fixed limit",
    )
}

pub(super) fn unsupported() -> PlatformError {
    PlatformError::new(
        ErrorCode::BindingCapabilityUnsupported,
        "AI Search operation is unsupported by this binding or model contract",
    )
}

pub(super) fn unavailable() -> PlatformError {
    PlatformError::new(
        ErrorCode::ResourceUnavailable,
        "AI Search execution is unavailable",
    )
}

pub(super) fn query_timeout() -> PlatformError {
    PlatformError::new(
        ErrorCode::ResourceLimitExceeded,
        "AI Search query exceeded its end-to-end deadline",
    )
}

pub(super) fn corrupt() -> PlatformError {
    PlatformError::new(
        ErrorCode::ResourceInvariantViolation,
        "AI Search authority is corrupt",
    )
}

pub(super) fn not_found() -> PlatformError {
    PlatformError::new(
        ErrorCode::ResourceNotFound,
        "AI Search object was not found",
    )
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ListInstances {
    pub(super) page: Option<u64>,
    pub(super) per_page: Option<u32>,
    pub(super) search: Option<String>,
    pub(super) order_by: Option<String>,
    pub(super) order_by_direction: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct DeleteInstance {
    pub(super) instance: String,
}
