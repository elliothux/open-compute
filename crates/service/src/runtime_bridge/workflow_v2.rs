//! Capability V2 dispatch and explicit durable suspension observations.

use super::*;
use open_compute_storage::WorkflowTarget;

/// Token-free result of one capability V2 activation.
#[derive(Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkflowV2Outcome {
    /// The run returned after traversing its registered history.
    Complete {
        /// Number of descriptors visited by this activation.
        #[serde(rename = "finalOrdinal")]
        final_ordinal: u32,
        /// Canonical final output, still checked against durable authority.
        #[serde(rename = "outputJson")]
        output_json: String,
    },
    /// A sanitized, deterministic failure, distinct from transport Unknown.
    Errored {
        /// Number of descriptors visited by this activation.
        #[serde(rename = "finalOrdinal")]
        final_ordinal: u32,
        /// Stable failure category; never contains a tenant exception.
        #[serde(rename = "errorCode")]
        error_code: String,
    },
    /// The trusted controller committed a yield and the dispatch RPC ended.
    Suspended {
        /// Number of descriptors visited before yielding.
        #[serde(rename = "finalOrdinal")]
        final_ordinal: u32,
    },
    /// A callback exceeded the trusted drain bound. No terminal or yield commit
    /// was made; new Workflow admission must remain fenced until runtime rotation.
    Unknown {
        /// Number of descriptors visited before the drain failed.
        #[serde(rename = "finalOrdinal")]
        final_ordinal: u32,
    },
}

impl std::fmt::Debug for WorkflowV2Outcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Complete { .. } => "WorkflowV2Outcome::Complete([REDACTED])",
            Self::Errored { .. } => "WorkflowV2Outcome::Errored([REDACTED])",
            Self::Suspended { .. } => "WorkflowV2Outcome::Suspended",
            Self::Unknown { .. } => "WorkflowV2Outcome::Unknown",
        })
    }
}

/// A generation-bound V2 response; suspension never implies terminal release.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkflowV2DispatchResult {
    #[serde(skip)]
    credential: Option<open_compute_runtime::GenerationCredential>,
    /// Durable control or business result observed by the system host.
    pub result: WorkflowV2Outcome,
    /// A logically fenced callback did not acknowledge completion before return.
    /// The driver must stop new Workflow admission rather than accumulate stragglers.
    pub drain_incomplete: bool,
    /// Whether the frozen V2 loader entry was cold or warm.
    pub loader_outcome: String,
}

impl std::fmt::Debug for WorkflowV2DispatchResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkflowV2DispatchResult")
            .field("drain_incomplete", &self.drain_incomplete)
            .field("loader_outcome", &self.loader_outcome)
            .finish_non_exhaustive()
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct VersionEnvelope<'a> {
    version_descriptor_sha256: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    activation_budget_ms: Option<u64>,
    #[serde(flatten, skip_serializing_if = "Option::is_none")]
    run: Option<&'a WorkflowRunRequest>,
}

fn dispatch_target(
    version: &WorkflowTarget,
    generation: i64,
) -> Result<DispatchTarget, PlatformError> {
    if version.capability_version != 2 {
        return Err(PlatformError::new(
            ErrorCode::WorkflowVersionNotReady,
            "Workflow execution capability is invalid",
        ));
    }
    Ok(DispatchTarget {
        account_id: version.account_id,
        worker_id: version.worker_id,
        deployment_id: version.deployment_id,
        worker_code_sha256: hex::encode(version.worker_code_sha256),
        entrypoint: Some(version.class_name.clone()),
        route_generation: generation,
        request_id: RequestId::generate(),
    })
}

impl WorkerdTransport {
    /// Check the shared runtime fence before claiming another durable activation.
    pub(crate) fn ensure_workflow_admission(&self) -> Result<(), PlatformError> {
        self.admit_workflow_v2().map(|_| ())
    }

    /// Probe the actual V2 runner for an immutable version before publication.
    pub async fn probe_workflow_v2(&self, version: &WorkflowTarget) -> Result<(), PlatformError> {
        let target = dispatch_target(version, 1)?;
        let (reply, _): (serde_json::Value, _) = tokio::time::timeout(
            Duration::from_secs(10),
            self.custom_event_request(
                "/internal/validate-workflow-v2",
                &target,
                &VersionEnvelope {
                    version_descriptor_sha256: hex::encode(version.descriptor_sha256),
                    activation_budget_ms: None,
                    run: None,
                },
                1024,
                None,
            ),
        )
        .await
        .map_err(|_| workflow_unavailable())??;
        if reply.get("valid") != Some(&serde_json::Value::Bool(true)) {
            return Err(workflow_unavailable());
        }
        Ok(())
    }

    /// Dispatch a frozen V2 version with a system-isolate controller and watchdog.
    pub async fn dispatch_workflow_v2(
        &self,
        version: &WorkflowTarget,
        request: &WorkflowRunRequest,
        timeout: Duration,
    ) -> Result<WorkflowV2DispatchResult, PlatformError> {
        let target = dispatch_target(version, request.fence.instance_generation)?;
        let admitted_generation = self.admit_workflow_v2()?;
        let received: Result<(WorkflowV2DispatchResult, _), _> = tokio::time::timeout(
            timeout,
            self.custom_event_request(
                "/internal/workflow-v2",
                &target,
                &VersionEnvelope {
                    version_descriptor_sha256: hex::encode(version.descriptor_sha256),
                    activation_budget_ms: Some(
                        u64::try_from(timeout.as_millis()).map_err(|_| workflow_unavailable())?,
                    ),
                    run: Some(request),
                },
                2 * 1024 * 1024 + 8192,
                Some(&admitted_generation),
            ),
        )
        .await
        .map_err(|_| workflow_unavailable())
        .and_then(std::convert::identity);
        let (mut response, credential) = match received {
            Ok(value) => value,
            Err(error) => {
                self.quarantine_workflow_v2(&admitted_generation)?;
                return Err(match error.code() {
                    ErrorCode::ArtifactIntegrityError
                    | ErrorCode::WorkflowInvariantViolation
                    | ErrorCode::WorkflowVersionNotReady => error,
                    _ => workflow_unavailable(),
                });
            }
        };
        if !matches!(response.loader_outcome.as_str(), "cold" | "warm") {
            self.quarantine_workflow_v2(&credential)?;
            return Err(workflow_unavailable());
        }
        let valid = match &response.result {
            WorkflowV2Outcome::Complete {
                final_ordinal,
                output_json,
            } => {
                *final_ordinal <= 1024
                    && output_json.len() <= open_compute_core::workflow::WORKFLOW_JSON_MAX_BYTES
            }
            WorkflowV2Outcome::Errored {
                final_ordinal,
                error_code,
            } => {
                *final_ordinal <= 1024
                    && open_compute_core::workflow::terminal_error_code_v2(error_code).is_ok()
            }
            WorkflowV2Outcome::Suspended { final_ordinal }
            | WorkflowV2Outcome::Unknown { final_ordinal } => *final_ordinal <= 1024,
        };
        if !valid {
            self.quarantine_workflow_v2(&credential)?;
            return Err(workflow_unavailable());
        }
        if response.drain_incomplete != matches!(response.result, WorkflowV2Outcome::Unknown { .. })
        {
            self.quarantine_workflow_v2(&credential)?;
            return Err(workflow_unavailable());
        }
        if response.drain_incomplete {
            self.quarantine_workflow_v2(&credential)?;
        }
        response.credential = Some(credential);
        Ok(response)
    }

    /// Fence a terminal or suspended V2 observation against the exact runtime generation that produced it.
    pub(crate) fn commit_workflow_v2<T>(
        &self,
        response: WorkflowV2DispatchResult,
        commit: impl FnOnce(WorkflowV2Outcome) -> Result<T, PlatformError>,
    ) -> Result<T, PlatformError> {
        let credential = response
            .credential
            .as_ref()
            .ok_or_else(workflow_unavailable)?;
        self.auth
            .with_current(credential, || commit(response.result))
            .unwrap_or_else(|| {
                Err(PlatformError::new(
                    ErrorCode::WorkflowRunStale,
                    "Workflow generation is stale",
                ))
            })
    }

    fn admit_workflow_v2(
        &self,
    ) -> Result<open_compute_runtime::GenerationCredential, PlatformError> {
        // Compilation installs credentials before readiness. Their presence
        // alone must not admit a claim or quarantine a not-yet-running child.
        let (_, current) = self.endpoint().map_err(|_| workflow_unavailable())?;
        let mut quarantine = self
            .workflow_v2_quarantine
            .lock()
            .map_err(|_| workflow_unavailable())?;
        if quarantine
            .as_ref()
            .is_some_and(|old| self.auth.with_current(old, || ()).is_some())
        {
            return Err(workflow_unavailable());
        }
        // Only a real supervised generation rotation releases quarantine. An
        // operator retry or another successful dispatch cannot erase it.
        *quarantine = None;
        Ok(current)
    }

    fn quarantine_workflow_v2(
        &self,
        credential: &open_compute_runtime::GenerationCredential,
    ) -> Result<(), PlatformError> {
        let mut quarantine = self
            .workflow_v2_quarantine
            .lock()
            .map_err(|_| workflow_unavailable())?;
        let _ = self.auth.with_current(credential, || {
            *quarantine = Some(credential.clone());
        });
        Ok(())
    }
}

fn workflow_unavailable() -> PlatformError {
    PlatformError::new(
        ErrorCode::WorkflowRuntimeUnavailable,
        "Workflow dispatch outcome is unknown",
    )
}
