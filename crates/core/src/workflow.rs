//! Workflow validation, private fences, local capacity, and frozen execution policy.

use crate::{ErrorCode, PlatformError, WorkflowInstanceId};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

mod json;
pub use json::{canonical_json, decode_json};
mod event;
pub use event::{WORKFLOW_EVENT_ENVELOPE_MAX_BYTES, WorkflowEventEnvelope};
mod descriptor;
mod duration;
mod policy;
pub use descriptor::{
    WORKFLOW_V2_DEPENDENCY_BYTES, WORKFLOW_V2_EVENT_BYTES, WORKFLOW_V2_INSTANCE_BYTES,
    WORKFLOW_V2_STEP_BYTES, WorkflowDurableConfig, WorkflowStepDeclaration, WorkflowStepDescriptor,
    WorkflowStepKind, validate_workflow_event_type,
};
pub use duration::{
    WORKFLOW_MAX_DURATION_MS, WORKFLOW_MAX_SAFE_INTEGER, duration_ms, timestamp_ms,
};
pub use policy::{
    WORKFLOW_DRAIN_MARGIN_MS, WORKFLOW_MAX_ATTEMPT_MS, WORKFLOW_MAX_RETRY_DELAY_MS,
    WorkflowBackoff, WorkflowRetention, WorkflowRetryPolicy, WorkflowStepConfig,
};

/// Maximum canonical input, step output, or final output bytes.
pub const WORKFLOW_JSON_MAX_BYTES: usize = 1024 * 1024;
/// Maximum JSON container nesting depth.
pub const WORKFLOW_JSON_MAX_DEPTH: usize = 127;

/// Local Workflow capacity and infrastructure recovery policy, not a Cloudflare plan.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct WorkflowsConfig {
    /// Maximum simultaneously active private Workflow backend requests.
    pub max_in_flight_requests: u32,
    /// Maximum descriptors retained by one instance.
    pub max_steps: u32,
    /// Input, descriptors, step results, and terminal state bytes per instance.
    pub max_state_bytes: u64,
    /// All retained instances per account, including terminal history.
    pub max_instances_per_account: u32,
    /// All retained instances per logical definition.
    pub max_instances_per_definition: u32,
    /// Nonterminal instances per account.
    pub max_active_per_account: u32,
    /// Total retained state bytes per account.
    pub max_account_state_bytes: u64,
    /// Lease duration for a live run.
    pub lease_ms: u64,
    /// Active transport heartbeat interval.
    pub heartbeat_ms: u64,
    /// Maximum time awaiting one workerd dispatch before Unknown recovery.
    pub dispatch_timeout_ms: u64,
    /// Minimum infrastructure backoff after lease expiry.
    pub recovery_backoff_ms: u64,
    /// Reservation age before an uncommitted creation may be released.
    pub creation_grace_ms: u64,
    /// Maximum callbacks granted in one immutable V2 batch, from one through sixteen.
    #[serde(skip_serializing_if = "default_parallel_steps")]
    pub max_parallel_steps: u32,
    /// Maximum unconsumed V2 events admitted for one instance.
    #[serde(skip_serializing_if = "default_buffered_events")]
    pub max_buffered_events: u32,
    /// Maximum logical inbox bytes, including event metadata.
    #[serde(skip_serializing_if = "default_event_bytes")]
    pub max_event_bytes: u64,
    /// Retention defaults adopted only when a new V2 instance omits an override.
    #[serde(skip_serializing_if = "default_retention")]
    pub default_retention: WorkflowRetention,
}

impl Default for WorkflowsConfig {
    fn default() -> Self {
        Self {
            max_in_flight_requests: 64,
            max_steps: 1024,
            max_state_bytes: 32 * 1024 * 1024,
            max_instances_per_account: 10000,
            max_instances_per_definition: 10000,
            max_active_per_account: 1000,
            max_account_state_bytes: 1024 * 1024 * 1024,
            lease_ms: 60000,
            heartbeat_ms: 20000,
            dispatch_timeout_ms: 300000,
            recovery_backoff_ms: 1000,
            creation_grace_ms: 60000,
            max_parallel_steps: 4,
            max_buffered_events: 128,
            max_event_bytes: 8 * 1024 * 1024,
            default_retention: WorkflowRetention::default(),
        }
    }
}

impl WorkflowsConfig {
    /// Reject inconsistent lease timing and unsafe or empty local budgets.
    pub fn validate(&self) -> Result<(), PlatformError> {
        self.default_retention.validate()?;
        if self.max_in_flight_requests == 0
            || self.max_in_flight_requests > 512
            || self.max_steps == 0
            || self.max_steps > 1024
            || self.max_state_bytes < WORKFLOW_JSON_MAX_BYTES as u64
            || self.max_state_bytes > 1024 * 1024 * 1024
            || self.max_instances_per_account == 0
            || self.max_instances_per_account > 1_000_000
            || self.max_instances_per_definition == 0
            || self.max_instances_per_definition > self.max_instances_per_account
            || self.max_active_per_account == 0
            || self.max_active_per_account > self.max_instances_per_account
            || self.max_account_state_bytes < self.max_state_bytes
            || self.max_account_state_bytes > 1024 * 1024 * 1024 * 1024
            || self.heartbeat_ms == 0
            || self.lease_ms > 300000
            || self.heartbeat_ms >= self.lease_ms / 2
            || self.lease_ms / 2 >= self.dispatch_timeout_ms
            || self.dispatch_timeout_ms > 3600000
            || self.recovery_backoff_ms == 0
            || self.recovery_backoff_ms > 60000
            || self.creation_grace_ms == 0
            || self.creation_grace_ms > 300000
            || !(1..=16).contains(&self.max_parallel_steps)
            || !(1..=128).contains(&self.max_buffered_events)
            || !(1..=8 * 1024 * 1024).contains(&self.max_event_bytes)
        {
            return Err(error(ErrorCode::LimitInvalid));
        }
        Ok(())
    }
}

// Serde's skip predicate borrows the field; omitting defaults preserves old snapshot fingerprints.
#[allow(clippy::trivially_copy_pass_by_ref)]
fn default_parallel_steps(value: &u32) -> bool {
    *value == 4
}
#[allow(clippy::trivially_copy_pass_by_ref)]
fn default_buffered_events(value: &u32) -> bool {
    *value == 128
}
#[allow(clippy::trivially_copy_pass_by_ref)]
fn default_event_bytes(value: &u64) -> bool {
    *value == 8 * 1024 * 1024
}

fn default_retention(value: &WorkflowRetention) -> bool {
    *value == WorkflowRetention::default()
}

/// A 256-bit private run, step, or creation fence. Debug never exposes its bytes.
#[derive(Clone, PartialEq, Eq)]
pub struct WorkflowToken([u8; 32]);

impl WorkflowToken {
    /// Wrap bytes generated by the owning repository's cryptographic RNG.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }
    /// Bytes for an exact SQLite token predicate; never log this value.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl std::fmt::Debug for WorkflowToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("WorkflowToken([REDACTED])")
    }
}

impl Serialize for WorkflowToken {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&hex::encode(self.0))
    }
}

impl<'de> Deserialize<'de> for WorkflowToken {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        if value.len() != 64
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(serde::de::Error::custom("invalid Workflow fence"));
        }
        let mut bytes = [0; 32];
        hex::decode_to_slice(value, &mut bytes)
            .map_err(|_| serde::de::Error::custom("invalid Workflow fence"))?;
        Ok(Self(bytes))
    }
}

/// Exact repository run mutation fence; the workerd generation is separately authenticated.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkflowFence {
    /// Durable internal instance identity.
    pub instance_id: WorkflowInstanceId,
    /// Exact immutable instance generation.
    pub instance_generation: i64,
    /// Current random run claim token.
    pub run_token: WorkflowToken,
}

/// Validate the capability's public instance identity using the upstream validator alphabet.
pub fn validate_workflow_instance_id(value: &str) -> Result<(), PlatformError> {
    let bytes = value.as_bytes();
    if bytes.is_empty()
        || bytes.len() > 100
        || !(bytes[0].is_ascii_alphanumeric() || bytes[0] == b'_')
        || !bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return Err(error(ErrorCode::WorkflowInstanceIdInvalid));
    }
    Ok(())
}

/// Validate an account-scoped logical Workflow display name.
pub fn validate_workflow_name(value: &str) -> Result<(), PlatformError> {
    if value.len() > 64 || validate_workflow_instance_id(value).is_err() {
        return Err(error(ErrorCode::WorkflowNotReady));
    }
    Ok(())
}

fn error(code: ErrorCode) -> PlatformError {
    PlatformError::new(code, "Workflow validation failed")
}

/// Decode only permanent Workflow failure categories emitted by the trusted dispatcher.
pub fn terminal_error_code(value: &str) -> Result<ErrorCode, PlatformError> {
    [
        ErrorCode::WorkflowExecutionFailed,
        ErrorCode::WorkflowNonDeterministic,
        ErrorCode::WorkflowStepConfigUnsupported,
        ErrorCode::WorkflowParallelStepUnsupported,
        ErrorCode::WorkflowMethodUnsupported,
        ErrorCode::WorkflowSerializationUnsupported,
        ErrorCode::WorkflowResultTooLarge,
        ErrorCode::WorkflowStateQuotaExceeded,
        ErrorCode::WorkflowStepLimitExceeded,
        ErrorCode::ArtifactIntegrityError,
    ]
    .into_iter()
    .find(|code| code.as_str() == value)
    .ok_or_else(|| error(ErrorCode::WorkflowInvariantViolation))
}

/// Decode the capability-two permanent failure vocabulary without broadening V1 history.
/// Transport Unknown, stale fences, and lifecycle conflicts are never settled business outcomes.
pub fn terminal_error_code_v2(value: &str) -> Result<ErrorCode, PlatformError> {
    match value {
        "WORKFLOW_STEP_TIMEOUT" => Ok(ErrorCode::WorkflowStepTimeout),
        "WORKFLOW_STEP_RETRIES_EXHAUSTED" => Ok(ErrorCode::WorkflowStepRetriesExhausted),
        "WORKFLOW_NON_RETRYABLE" => Ok(ErrorCode::WorkflowNonRetryable),
        "WORKFLOW_EVENT_TIMEOUT" => Ok(ErrorCode::WorkflowEventTimeout),
        "WORKFLOW_DURATION_INVALID" => Ok(ErrorCode::WorkflowDurationInvalid),
        "WORKFLOW_EVENT_TYPE_INVALID" => Ok(ErrorCode::WorkflowEventTypeInvalid),
        _ => terminal_error_code(value),
    }
}

#[cfg(test)]
#[path = "workflow_tests.rs"]
mod tests;
