use super::*;

pub(crate) fn response_error(code: ErrorCode) -> Response {
    let status = match code {
        ErrorCode::WorkflowRuntimeUnavailable
        | ErrorCode::WorkflowInvariantViolation
        | ErrorCode::StoragePressure
        | ErrorCode::PlatformUnavailable => StatusCode::SERVICE_UNAVAILABLE,
        ErrorCode::WorkflowInstanceAlreadyExists
        | ErrorCode::WorkflowInstanceStateConflict
        | ErrorCode::WorkflowInstanceBusy
        | ErrorCode::WorkflowInstanceCleanupPending
        | ErrorCode::WorkflowRunStale
        | ErrorCode::WorkflowStepStale => StatusCode::CONFLICT,
        ErrorCode::WorkflowNotFound | ErrorCode::WorkflowInstanceNotFound => StatusCode::NOT_FOUND,
        ErrorCode::WorkflowStateQuotaExceeded
        | ErrorCode::WorkflowStepLimitExceeded
        | ErrorCode::WorkflowEventQueueFull => StatusCode::TOO_MANY_REQUESTS,
        ErrorCode::WorkflowPayloadTooLarge | ErrorCode::WorkflowResultTooLarge => {
            StatusCode::PAYLOAD_TOO_LARGE
        }
        _ => StatusCode::UNPROCESSABLE_ENTITY,
    };
    let mut response = status.into_response();
    response.headers_mut().insert(
        HeaderName::from_static("x-open-compute-error-code"),
        HeaderValue::from_static(code.as_str()),
    );
    response
}
