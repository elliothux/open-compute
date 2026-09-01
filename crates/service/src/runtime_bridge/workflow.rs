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
    /// Bounded canonical durable-value payload.
    pub payload_base64: String,
    /// Whether this activation replays successful handlers and executes durable rollback.
    pub rollback: bool,
    /// Direct-cron metadata, absent for programmatic and REST-created instances.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schedule: Option<open_compute_core::WorkflowCronSchedule>,
}

impl std::fmt::Debug for WorkflowRunRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkflowRunRequest").finish_non_exhaustive()
    }
}

use open_compute_storage::WorkflowTarget;

/// Token-free result of one Workflow activation.
#[derive(Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkflowOutcome {
    /// The run returned after traversing its registered history.
    Complete {
        /// Number of descriptors visited by this activation.
        #[serde(rename = "finalOrdinal")]
        final_ordinal: u32,
        /// Canonical final durable-value output, still checked against authority.
        #[serde(rename = "outputBase64")]
        output_base64: String,
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
    /// Durable rollback replay completed or halted at its first exhausted handler.
    Terminated {
        /// One past the final replayed or rollback descriptor.
        #[serde(rename = "finalOrdinal")]
        final_ordinal: u32,
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

impl std::fmt::Debug for WorkflowOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Complete { .. } => "WorkflowOutcome::Complete([REDACTED])",
            Self::Errored { .. } => "WorkflowOutcome::Errored([REDACTED])",
            Self::Terminated { .. } => "WorkflowOutcome::Terminated",
            Self::Suspended { .. } => "WorkflowOutcome::Suspended",
            Self::Unknown { .. } => "WorkflowOutcome::Unknown",
        })
    }
}

/// A generation-bound response; suspension never implies terminal release.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkflowDispatchResult {
    #[serde(skip)]
    credential: Option<open_compute_runtime::GenerationCredential>,
    /// Durable control or business result observed by the system host.
    pub result: WorkflowOutcome,
    /// A logically fenced callback did not acknowledge completion before return.
    /// The driver must stop new Workflow admission rather than accumulate stragglers.
    pub drain_incomplete: bool,
    /// Whether the frozen loader entry was cold or warm.
    pub loader_outcome: String,
}

impl std::fmt::Debug for WorkflowDispatchResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkflowDispatchResult")
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
    if version.capability_version != 1 {
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
        self.admit_workflow().map(|_| ())
    }

    /// Probe the actual runner for an immutable version before publication.
    pub async fn probe_workflow(&self, version: &WorkflowTarget) -> Result<(), PlatformError> {
        let target = dispatch_target(version, 1)?;
        let (reply, _): (serde_json::Value, _) = tokio::time::timeout(
            Duration::from_secs(10),
            self.custom_event_request(
                "/internal/validate-workflow",
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

    /// Dispatch a frozen version with a system-isolate controller and watchdog.
    pub async fn dispatch_workflow(
        &self,
        version: &WorkflowTarget,
        request: &WorkflowRunRequest,
        timeout: Duration,
    ) -> Result<WorkflowDispatchResult, PlatformError> {
        let target = dispatch_target(version, request.fence.instance_generation)?;
        let admitted_generation = self.admit_workflow()?;
        let received: Result<(WorkflowDispatchResult, _), _> = tokio::time::timeout(
            timeout,
            self.custom_event_request(
                "/internal/workflow",
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
                self.quarantine_workflow(&admitted_generation)?;
                return Err(match error.code() {
                    ErrorCode::ArtifactIntegrityError
                    | ErrorCode::WorkflowInvariantViolation
                    | ErrorCode::WorkflowVersionNotReady => error,
                    _ => workflow_unavailable(),
                });
            }
        };
        if !matches!(response.loader_outcome.as_str(), "cold" | "warm") {
            self.quarantine_workflow(&credential)?;
            return Err(workflow_unavailable());
        }
        let valid = match &response.result {
            WorkflowOutcome::Complete {
                final_ordinal,
                output_base64,
            } => {
                *final_ordinal <= 1024
                    && open_compute_core::workflow::durable_value_base64(
                        output_base64,
                        ErrorCode::WorkflowResultTooLarge,
                    )
                    .is_ok()
            }
            WorkflowOutcome::Errored {
                final_ordinal,
                error_code,
            } => {
                *final_ordinal <= 1024
                    && open_compute_core::workflow::terminal_error_code(error_code).is_ok()
            }
            WorkflowOutcome::Terminated { final_ordinal } => *final_ordinal <= 1024,
            WorkflowOutcome::Suspended { final_ordinal }
            | WorkflowOutcome::Unknown { final_ordinal } => *final_ordinal <= 1024,
        };
        if !valid {
            self.quarantine_workflow(&credential)?;
            return Err(workflow_unavailable());
        }
        if response.drain_incomplete != matches!(response.result, WorkflowOutcome::Unknown { .. }) {
            self.quarantine_workflow(&credential)?;
            return Err(workflow_unavailable());
        }
        if response.drain_incomplete {
            self.quarantine_workflow(&credential)?;
        }
        response.credential = Some(credential);
        Ok(response)
    }

    /// Fence a terminal or suspended observation against the exact runtime generation that produced it.
    pub(crate) fn commit_workflow<T>(
        &self,
        response: WorkflowDispatchResult,
        commit: impl FnOnce(WorkflowOutcome) -> Result<T, PlatformError>,
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

    fn admit_workflow(&self) -> Result<open_compute_runtime::GenerationCredential, PlatformError> {
        // Compilation installs credentials before readiness. Their presence
        // alone must not admit a claim or quarantine a not-yet-running child.
        let (_, current) = self.endpoint().map_err(|_| workflow_unavailable())?;
        let mut quarantine = self
            .workflow_quarantine
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

    fn quarantine_workflow(
        &self,
        credential: &open_compute_runtime::GenerationCredential,
    ) -> Result<(), PlatformError> {
        let mut quarantine = self
            .workflow_quarantine
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

#[cfg(test)]
#[path = "workflow_tests.rs"]
mod tests;
