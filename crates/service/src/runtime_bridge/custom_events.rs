//! Authenticated, bounded native custom-event HTTP transport.

use super::*;

impl WorkerdTransport {
    /// Deliver one frozen Queue claim through workerd's native custom-event API.
    pub async fn dispatch_queue(
        &self,
        target: &DispatchTarget,
        request: &QueueDispatchRequest,
        timeout: Duration,
    ) -> Result<QueueDispatchResult, PlatformError> {
        validate_queue_dispatch_request(request)?;
        let (result, _) = tokio::time::timeout(
            timeout,
            self.custom_event_request(
                "/internal/queue",
                target,
                request,
                MAX_CUSTOM_EVENT_RESPONSE,
                None,
            ),
        )
        .await
        .map_err(|_| {
            PlatformError::new(
                ErrorCode::QueueSendResultUnknown,
                "Queue custom-event result is unknown",
            )
        })??;
        validate_queue_dispatch_result(result, request.messages.len())
    }

    /// Deliver one frozen Cron run through workerd's native scheduled API.
    pub async fn dispatch_scheduled(
        &self,
        target: &DispatchTarget,
        request: &ScheduledDispatchRequest,
        timeout: Duration,
    ) -> Result<ScheduledDispatchResult, PlatformError> {
        validate_scheduled_dispatch_request(request)?;
        let (result, _) = tokio::time::timeout(
            timeout,
            self.custom_event_request(
                "/internal/scheduled",
                target,
                request,
                MAX_CUSTOM_EVENT_RESPONSE,
                None,
            ),
        )
        .await
        .map_err(|_| {
            PlatformError::new(
                ErrorCode::SchedulerUnavailable,
                "scheduled custom-event result is unknown",
            )
        })??;
        validate_scheduled_dispatch_result(result)
    }

    pub(super) async fn custom_event_request<T: for<'de> Deserialize<'de>>(
        &self,
        path: &str,
        target: &DispatchTarget,
        body: &impl Serialize,
        max_response: usize,
        expected_generation: Option<&open_compute_runtime::GenerationCredential>,
    ) -> Result<(T, open_compute_runtime::GenerationCredential), PlatformError> {
        let (port, credential) = self.endpoint()?;
        if expected_generation
            .is_some_and(|expected| self.auth.with_current(expected, || ()).is_none())
        {
            return Err(runtime_unavailable());
        }
        if target.route_generation < 1 {
            return Err(custom_event_protocol_error());
        }
        let bytes = serde_json::to_vec(body).map_err(|_| custom_event_protocol_error())?;
        let mut request = hyper::Request::builder()
            .method(Method::POST)
            .uri(format!("http://127.0.0.1:{port}{path}"))
            .header(TOKEN_HEADER, credential.expose())
            .header(header::CONTENT_TYPE, "application/json")
            .header("x-open-compute-account-id", target.account_id.to_string())
            .header("x-open-compute-worker-id", target.worker_id.to_string())
            .header(
                "x-open-compute-deployment-id",
                target.deployment_id.to_string(),
            )
            .header("x-open-compute-loader-key", target.loader_key())
            .header(
                "x-open-compute-worker-code-sha256",
                &target.worker_code_sha256,
            )
            .header(
                "x-open-compute-route-generation",
                target.route_generation.to_string(),
            )
            .header("x-open-compute-request-id", target.request_id.to_string());
        if let Some(entrypoint) = &target.entrypoint {
            request = request.header("x-open-compute-entrypoint", entrypoint);
        }
        let request = request
            .body(Body::from(bytes))
            .map_err(|_| custom_event_protocol_error())?;
        let response = self
            .body_client
            .request(request)
            .await
            .map_err(|_| runtime_unavailable())?;
        if !response.status().is_success() {
            if matches!(path, "/internal/workflow" | "/internal/validate-workflow") {
                let code = if response.status() == StatusCode::UNPROCESSABLE_ENTITY {
                    match response
                        .headers()
                        .get(ERROR_HEADER)
                        .and_then(|header| header.to_str().ok())
                    {
                        Some("ARTIFACT_INTEGRITY_ERROR") => ErrorCode::ArtifactIntegrityError,
                        Some("WORKFLOW_INVARIANT_VIOLATION") => {
                            ErrorCode::WorkflowInvariantViolation
                        }
                        _ => ErrorCode::WorkflowVersionNotReady,
                    }
                } else {
                    ErrorCode::WorkflowRuntimeUnavailable
                };
                return Err(PlatformError::new(code, "Workflow dispatch failed"));
            }
            return Err(PlatformError::new(
                if path == "/internal/queue" {
                    ErrorCode::QueueCustomEventUnsupported
                } else {
                    ErrorCode::CronCustomEventUnsupported
                },
                "native custom-event dispatch failed",
            ));
        }
        let bytes = to_bytes(Body::new(response.into_body()), max_response)
            .await
            .map_err(|_| custom_event_protocol_error())?;
        let value = serde_json::from_slice(&bytes).map_err(|_| custom_event_protocol_error())?;
        if self.auth.with_current(&credential, || ()).is_none() {
            return Err(runtime_unavailable());
        }
        Ok((value, credential))
    }
}
