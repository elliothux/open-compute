use crate::{WorkflowInstanceIdentity, WorkflowTarget};
use open_compute_core::{WorkflowFence, WorkflowInstanceId, WorkflowToken};
use serde::{Deserialize, Serialize};

/// Durable Workflow instance execution state.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowState {
    /// Waiting for pool admission.
    Queued,
    /// A random, unexpired lease owns activation.
    Running,
    /// No activation is leased while a durable deadline or event is pending.
    Waiting,
    /// Explicitly paused; wall-clock deadlines continue to pass.
    Paused,
    /// All steps and final output committed.
    Complete,
    /// A known permanent failure committed.
    Errored,
    /// Explicitly terminated and logically fenced, with retained history.
    Terminated,
}

impl WorkflowState {
    /// Whether this execution is terminal; only an explicit V2 restart can make it runnable again.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Complete | Self::Errored | Self::Terminated)
    }
}

/// Disposable admission position. Losing it never loses work from the durable ready index.
#[derive(Clone, Debug, Default)]
pub struct WorkflowClaimCursor {
    pub(super) account: Option<open_compute_core::AccountId>,
    pub(super) recovered_streak: u8,
}

/// Persisted sanitized exception record, never raw tenant exception text.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowFailure {
    /// Safe exception category.
    pub name: String,
    /// Stable public explanation without tenant values or internal topology.
    pub message: String,
}

impl Default for WorkflowFailure {
    fn default() -> Self {
        Self {
            name: "Error".into(),
            message: "Workflow execution failed".into(),
        }
    }
}

/// One immutable-instance record. Diagnostic formatting excludes payload, result, and tokens.
#[derive(Clone)]
pub struct WorkflowInstanceRecord {
    /// Cross-database immutable identity, not control reservation lifecycle state.
    pub identity: WorkflowInstanceIdentity,
    /// Scheduler execution state.
    pub state: WorkflowState,
    /// Canonical durable input, never an operator inspect field.
    pub input_json: String,
    /// Canonical durable success output.
    pub output_json: Option<String>,
    /// Sanitized terminal failure.
    pub error: Option<WorkflowFailure>,
    /// Low-cardinality terminal failure category.
    pub error_code: Option<String>,
    /// Current run claim token, if leased.
    pub run_token: Option<WorkflowToken>,
    /// Lease deadline.
    pub run_lease_until_ms: Option<i64>,
    /// Next eligible activation time.
    pub next_run_at_ms: Option<i64>,
    /// Contiguous successfully committed step frontier.
    pub completed_step_count: u32,
    /// Logical retained payload, descriptor, result, and error bytes.
    pub state_bytes: u64,
    /// Terminal transition timestamp.
    pub terminal_at_ms: Option<i64>,
    /// Last durable scheduler mutation.
    pub updated_at_ms: i64,
    /// Capability-two scheduling, accounting and lifecycle authority; absent for V1 history.
    pub durable: Option<WorkflowDurableState>,
}

/// Persisted capability-two metadata, with no payloads or private run/creation tokens.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowDurableState {
    /// The active run must drain before transitioning to paused.
    pub pause_requested: bool,
    /// The active run must drain before releasing its lease.
    pub yield_requested: bool,
    /// Earliest unfinished attempt, retry or wait deadline.
    pub next_wake_at_ms: Option<i64>,
    /// Number of immutable descriptors in this generation.
    pub registered_step_count: u32,
    /// Number of complete or failed descriptors; cancelled descriptors are not settled.
    pub settled_step_count: u32,
    /// Policy frozen when the instance was created.
    pub retention: open_compute_core::workflow::WorkflowRetention,
    /// Terminal expiry, never extended by reads.
    pub expires_at_ms: Option<i64>,
    /// Exact operation whose restart committed this execution generation.
    pub last_restart_operation_id: Option<open_compute_core::WorkflowOperationId>,
    /// Number of unconsumed inbox events.
    pub event_count: u32,
    /// Logical bytes retained in the inbox, including event metadata.
    pub event_bytes: u64,
    /// Next monotonic FIFO sequence; consumption does not reuse previous numbers.
    pub next_event_seq: i64,
    /// Whether this instance has previously been admitted, used for recovery fairness.
    pub has_activated: bool,
}

impl std::fmt::Debug for WorkflowInstanceRecord {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkflowInstanceRecord")
            .field("id", &self.identity.instance_id)
            .field("state", &self.state)
            .field("completed_steps", &self.completed_step_count)
            .finish_non_exhaustive()
    }
}

/// A durably claimed run assembled only from immutable scheduler authority.
#[derive(Clone)]
pub struct ClaimedWorkflowRun {
    /// Exact run mutation fence.
    pub fence: WorkflowFence,
    /// Frozen deployment and class.
    pub target: WorkflowTarget,
    /// Public definition-scoped instance identity.
    pub external_instance_id: String,
    /// Stable event creation timestamp.
    pub created_at_ms: i64,
    /// Canonical durable input.
    pub input_json: String,
}

impl std::fmt::Debug for ClaimedWorkflowRun {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClaimedWorkflowRun")
            .field("fence", &self.fence)
            .finish_non_exhaustive()
    }
}

/// Exact sequential replay identity submitted by the trusted callback controller.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkflowStepIdentity {
    /// Zero-based position within this activation.
    pub ordinal: u32,
    /// Validated user step name.
    pub name: String,
    /// One-based count of this name in the replay sequence.
    pub name_count: u32,
    /// V1 accepts only the canonical JSON string `null`.
    pub config_json: String,
}

/// Private persistence reply. Execution grants never reach tenant code.
#[derive(Clone, Serialize)]
#[serde(
    tag = "state",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum WorkflowStepGrant {
    /// Callback may execute under this private step token.
    Run {
        /// Unpredictable exact step mutation fence.
        step_token: WorkflowToken,
    },
    /// Durable success; callback must not execute.
    Complete {
        /// Canonical result to parse inside the tenant realm.
        output_json: String,
    },
    /// Durable failure; callback must not execute.
    Failed {
        /// Sanitized exception to rethrow.
        error: WorkflowFailure,
        /// Original low-cardinality permanent failure category.
        error_code: String,
    },
}

impl std::fmt::Debug for WorkflowStepGrant {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Run { .. } => "Run([REDACTED])",
            Self::Complete { .. } => "Complete([REDACTED])",
            Self::Failed { .. } => "Failed",
        })
    }
}

/// A known final execution outcome, never an infrastructure timeout or transport failure.
#[derive(Clone)]
pub enum WorkflowCompletion {
    /// Canonical final output plus the number of exact step descriptors visited this run.
    Complete {
        /// Untrusted serialized output, validated again before committing.
        output_json: String,
        /// One past the last descriptor visited by the trusted controller.
        final_ordinal: u32,
    },
    /// Permanent failure without raw tenant exception text.
    Errored {
        /// Stable category from the trusted dispatcher.
        code: open_compute_core::ErrorCode,
    },
}

impl std::fmt::Debug for WorkflowCompletion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Complete { final_ordinal, .. } => f
                .debug_struct("Complete")
                .field("final_ordinal", final_ordinal)
                .finish_non_exhaustive(),
            Self::Errored { code } => f.debug_struct("Errored").field("code", code).finish(),
        }
    }
}

/// Secret-free Workflow health aggregates and bounded integrity inspection results.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowInspection {
    /// Queued instances including future backoff.
    pub queued: u64,
    /// Currently leased instances.
    pub running: u64,
    /// Durable successes, including retained V2 history.
    pub complete: u64,
    /// Durable known failures, including retained V2 history.
    pub errored: u64,
    /// Persisted logical state bytes across all accounts.
    pub state_bytes: u64,
    /// Expired runs awaiting bounded recovery.
    pub expired_runs: u64,
    /// Durable waits without an activation lease.
    pub waiting: u64,
    /// Explicitly paused instances, still charged to nonterminal quota.
    pub paused: u64,
    /// Explicitly terminated instances.
    pub terminated: u64,
    /// Terminal V2 histories pending retention expiry or purge.
    pub retained: u64,
    /// Unconsumed events across retained generations.
    pub buffered_events: u64,
    /// Inbox logical bytes, excluding already-consumed payloads.
    pub inbox_bytes: u64,
    /// Consumed events in the retained generations, reset by restart/purge.
    pub consumed_events: u64,
    /// Unfinished durable sleep descriptors, including paused waits.
    pub sleeping_steps: u64,
    /// Unfinished event waits, including paused waits.
    pub event_waits: u64,
    /// Descriptors waiting for a business retry, including paused instances.
    pub retry_waits: u64,
    /// Completed descriptors that needed more than one business attempt.
    pub retried_steps: u64,
    /// Descriptors with an exhausted retry policy in retained history.
    pub exhausted_steps: u64,
    /// Retained settled attempt-timeout failures.
    pub step_timeouts: u64,
    /// Retained settled event-timeout failures.
    pub event_timeouts: u64,
    /// Durable purge receipts not yet acknowledged and swept.
    pub gc_receipts: u64,
}

/// Bounded operator metadata; never includes input, output, nonce, or claim tokens.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowInstanceInspection {
    /// Internal immutable identity.
    pub id: WorkflowInstanceId,
    /// Public definition-scoped identity.
    pub external_instance_id: String,
    /// Frozen version identity.
    pub version_id: open_compute_core::WorkflowVersionId,
    /// Frozen deployment identity.
    pub deployment_id: open_compute_core::DeploymentId,
    /// Frozen named export.
    pub class_name: String,
    /// Instance mutation generation.
    pub generation: i64,
    /// Persisted execution state.
    pub status: WorkflowState,
    /// Committed successful step frontier.
    pub completed_step_count: u32,
    /// Total persisted step descriptors.
    pub step_count: u32,
    /// Retained logical bytes.
    pub state_bytes: u64,
    /// Remaining lease, zero when expired, absent without a claim.
    pub lease_remaining_ms: Option<i64>,
    /// Immutable creation time.
    pub created_at_ms: i64,
    /// Terminal timestamp when present.
    pub terminal_at_ms: Option<i64>,
    /// Sanitized terminal category.
    pub error_code: Option<String>,
    /// Frozen execution capability; V1 history is never implicitly upgraded.
    pub capability_version: u32,
    /// Persisted waiting, retention and inbox metadata; absent for V1 history.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub durable: Option<WorkflowDurableState>,
}

/// Authenticated operator step metadata, excluding outputs and private token material.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowStepInspection {
    /// Owning internal instance identity.
    pub instance_id: WorkflowInstanceId,
    /// Sequential position.
    pub ordinal: u32,
    /// Step display name.
    pub name: String,
    /// Name occurrence count.
    pub name_count: u32,
    /// Durable step status.
    pub state: String,
    /// Output size, without content.
    pub output_bytes: u64,
    /// Low-cardinality failure category, without raw exception text.
    pub error_code: Option<String>,
    /// Durable operation kind, excluding its configuration and payload.
    pub kind: String,
    /// One-based business attempt, or zero before the first callback grant.
    pub attempt: u32,
    /// Frozen current-attempt deadline, if a business attempt exists.
    pub attempt_deadline_at_ms: Option<i64>,
    /// Original absolute retry, sleep or event deadline.
    pub due_at_ms: Option<i64>,
    /// First ordinal of the immutable V2 batch; absent for V1 history.
    pub batch_first_ordinal: Option<u32>,
    /// Immutable V2 batch size; absent for V1 history.
    pub batch_size: Option<u32>,
}
