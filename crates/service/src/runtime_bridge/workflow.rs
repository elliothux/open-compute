//! Frozen Workflow dispatch on the authenticated dynamic-loader transport.

use super::*;

/// Host-only run envelope. Tokens are excluded from diagnostic formatting.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowRunRequest {
    /// Exact durable instance/run mutation fence, private to trusted transport.
    #[serde(flatten)]
    pub fence: open_compute_core::WorkflowFence,
    /// Public definition-scoped instance identity.
    pub external_instance_id: String,
    /// Definition name frozen at creation.
    pub definition_name: String,
    /// Durable creation timestamp.
    pub created_at_ms: i64,
    /// Bounded canonical JSON payload.
    pub payload_json: String,
}

impl std::fmt::Debug for WorkflowRunRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkflowRunRequest").finish_non_exhaustive()
    }
}

/// Trusted dispatcher observation; a transport error is always an Unknown run.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkflowDispatchResult {
    #[serde(skip)]
    credential: Option<open_compute_runtime::GenerationCredential>,
    /// `complete` or `errored`, never a tenant-supplied dispatch status.
    pub outcome: String,
    /// Number of step descriptors visited by this activation.
    pub final_ordinal: u32,
    /// Canonical success value.
    pub output_json: Option<String>,
    /// Sanitized failure category.
    pub error_code: Option<String>,
    /// Sanitized failure record, without tenant stack or internal identity.
    pub error: Option<open_compute_storage::scheduler::WorkflowFailure>,
    /// Observation of the immutable dynamic-loader cache.
    pub loader_outcome: String,
}

impl std::fmt::Debug for WorkflowDispatchResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkflowDispatchResult")
            .field("outcome", &self.outcome)
            .field("final_ordinal", &self.final_ordinal)
            .field("error_code", &self.error_code)
            .finish_non_exhaustive()
    }
}

impl WorkerdTransport {
    /// Check inheritance and the `run` method without invoking tenant code.
    pub async fn probe_workflow(&self, target: &DispatchTarget) -> Result<(), PlatformError> {
        let (reply, _): (serde_json::Value, _) = tokio::time::timeout(
            Duration::from_secs(10),
            self.custom_event_request("/internal/validate-workflow", target, &(), 1024, None),
        )
        .await
        .map_err(|_| runtime_unavailable())??;
        if reply.get("valid") != Some(&serde_json::Value::Bool(true)) {
            return Err(runtime_unavailable());
        }
        Ok(())
    }

    /// Run a frozen class with a callback-aware facade in the same tenant realm.
    pub async fn dispatch_workflow(
        &self,
        target: &DispatchTarget,
        request: &WorkflowRunRequest,
        timeout: Duration,
    ) -> Result<WorkflowDispatchResult, PlatformError> {
        let (mut result, credential): (WorkflowDispatchResult, _) = tokio::time::timeout(
            timeout,
            self.custom_event_request(
                "/internal/workflow",
                target,
                request,
                2 * 1024 * 1024 + 8192,
                None,
            ),
        )
        .await
        .map_err(|_| runtime_unavailable())??;
        if !matches!(result.outcome.as_str(), "complete" | "errored")
            || !matches!(result.loader_outcome.as_str(), "cold" | "warm")
            || (result.outcome == "complete") != result.output_json.is_some()
            || (result.outcome == "errored") != result.error.is_some()
            || (result.outcome == "errored") != result.error_code.is_some()
        {
            return Err(runtime_unavailable());
        }
        if result.error.as_ref().is_some_and(|error| {
            *error != open_compute_storage::scheduler::WorkflowFailure::default()
        }) {
            return Err(runtime_unavailable());
        }
        result.credential = Some(credential);
        Ok(result)
    }

    /// Serialize a short durable completion with generation rotation, without exposing its credential.
    pub(crate) fn commit_workflow<T>(
        &self,
        mut response: WorkflowDispatchResult,
        commit: impl FnOnce(WorkflowDispatchResult) -> Result<T, PlatformError>,
    ) -> Result<T, PlatformError> {
        let credential = response.credential.take().ok_or_else(runtime_unavailable)?;
        self.auth
            .with_current(&credential, || commit(response))
            .unwrap_or_else(|| {
                Err(PlatformError::new(
                    ErrorCode::WorkflowRunStale,
                    "Workflow generation is stale",
                ))
            })
    }
}

#[cfg(test)]
#[path = "workflow_tests.rs"]
mod tests;
