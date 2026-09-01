//! Current replay identity, resolved wait policy, and logical byte accounting.

use super::{
    WORKFLOW_MAX_DURATION_MS, WORKFLOW_MAX_SAFE_INTEGER, WorkflowStepConfig, duration_ms, error,
    timestamp_ms,
};
use crate::{ErrorCode, PlatformError};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};

/// Fixed instance metadata charge defined by `share/workflow-accounting.json`.
pub const WORKFLOW_INSTANCE_BYTES: usize = 256;
/// Fixed step metadata charge, excluding variable name/config/result bytes.
pub const WORKFLOW_STEP_BYTES: usize = 160;
/// Logical bytes per immutable predecessor edge.
pub const WORKFLOW_DEPENDENCY_BYTES: usize = 16;
/// Fixed inbox metadata charge, excluding type and canonical payload bytes.
pub const WORKFLOW_EVENT_BYTES: usize = 32;

/// Durable API operation kind, independent of the caller's display name.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowStepKind {
    /// Retried business callback.
    Do,
    /// Relative durable sleep.
    Sleep,
    /// Absolute durable sleep.
    SleepUntil,
    /// FIFO event wait with a durable timeout.
    WaitEvent,
}

/// Public restart selector kind; both relative and absolute sleeps share Cloudflare's `sleep` kind.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub enum WorkflowRestartStepType {
    /// A `WorkflowStep.do` callback.
    #[serde(rename = "do")]
    Do,
    /// Either `sleep` or `sleepUntil`.
    #[serde(rename = "sleep")]
    Sleep,
    /// A `waitForEvent` step.
    #[serde(rename = "waitForEvent")]
    WaitForEvent,
}

impl WorkflowRestartStepType {
    /// Stable control-authority spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Do => "do",
            Self::Sleep => "sleep",
            Self::WaitForEvent => "waitForEvent",
        }
    }

    /// Whether an internal durable step kind belongs to this public selector kind.
    #[must_use]
    pub const fn matches(self, kind: WorkflowStepKind) -> bool {
        matches!(
            (self, kind),
            (Self::Do, WorkflowStepKind::Do)
                | (
                    Self::Sleep,
                    WorkflowStepKind::Sleep | WorkflowStepKind::SleepUntil
                )
                | (Self::WaitForEvent, WorkflowStepKind::WaitEvent)
        )
    }
}

/// Exact one-based step occurrence from which a new Workflow generation must resume.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkflowRestartSelector {
    /// Step name as supplied to the Workflow API.
    pub name: String,
    /// One-based occurrence for the selected name and kind.
    #[serde(default = "first_occurrence")]
    pub count: u32,
    /// Optional disambiguation when multiple step kinds use the same name and occurrence.
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub step_type: Option<WorkflowRestartStepType>,
}

impl WorkflowRestartSelector {
    /// Validate the strict pinned selector without normalizing its identity.
    pub fn validate(&self) -> Result<(), PlatformError> {
        if self.name.is_empty() || self.name.len() > 256 || self.count == 0 || self.count > 1024 {
            return Err(error(ErrorCode::WorkflowMethodUnsupported));
        }
        Ok(())
    }
}

const fn first_occurrence() -> u32 {
    1
}

impl WorkflowStepKind {
    /// Stable SQL/wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Do => "do",
            Self::Sleep => "sleep",
            Self::SleepUntil => "sleep_until",
            Self::WaitEvent => "wait_event",
        }
    }
}

/// Fully resolved configuration. Variant identity prevents mismatched kind/config pairs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WorkflowDurableConfig {
    /// Immutable callback retry and timeout policy.
    Do(WorkflowStepConfig),
    /// Normalized relative milliseconds.
    Sleep(u64),
    /// Absolute safe integral Unix milliseconds, including past timestamps.
    SleepUntil(i64),
    /// Event type and normalized timeout; neither comes from runtime cache state.
    WaitEvent {
        /// Exact ASCII event type, compared case-sensitively.
        event_type: String,
        /// Timeout in milliseconds, including zero.
        timeout_ms: u64,
    },
}

impl WorkflowDurableConfig {
    /// Normalize one public config object; missing defaults are resolved only here.
    pub fn resolve(kind: WorkflowStepKind, value: &Value) -> Result<Self, PlatformError> {
        let resolved = match kind {
            WorkflowStepKind::Do => Self::Do(WorkflowStepConfig::resolve(value)?),
            WorkflowStepKind::Sleep => {
                fields(value, &["duration"])?;
                Self::Sleep(duration_ms(
                    required(value, "duration")?,
                    WORKFLOW_MAX_DURATION_MS,
                )?)
            }
            WorkflowStepKind::SleepUntil => {
                fields(value, &["timestamp"])?;
                Self::SleepUntil(timestamp_ms(required(value, "timestamp")?)?)
            }
            WorkflowStepKind::WaitEvent => {
                fields(value, &["type", "timeout"])?;
                Self::WaitEvent {
                    event_type: required(value, "type")?
                        .as_str()
                        .ok_or_else(|| error(ErrorCode::WorkflowEventTypeInvalid))?
                        .into(),
                    timeout_ms: value.get("timeout").map_or(Ok(86_400_000), |value| {
                        duration_ms(value, WORKFLOW_MAX_DURATION_MS)
                    })?,
                }
            }
        };
        resolved.validate()?;
        Ok(resolved)
    }

    /// Decode stored resolved bytes, rejecting missing fields, noncanonical encodings, and corruption.
    /// This deliberately does not apply the public defaults again on replay.
    pub fn from_canonical(kind: WorkflowStepKind, encoded: &str) -> Result<Self, PlatformError> {
        let decode = || {
            if encoded.len() > 4096 {
                return Err(unsupported());
            }
            let value: Value = serde_json::from_str(encoded).map_err(|_| unsupported())?;
            let config = match kind {
                WorkflowStepKind::Do => {
                    Self::Do(serde_json::from_value(value).map_err(|_| unsupported())?)
                }
                WorkflowStepKind::Sleep => {
                    fields(&value, &["durationMs"])?;
                    Self::Sleep(
                        required(&value, "durationMs")?
                            .as_u64()
                            .ok_or_else(unsupported)?,
                    )
                }
                WorkflowStepKind::SleepUntil => {
                    fields(&value, &["timestampMs"])?;
                    Self::SleepUntil(
                        required(&value, "timestampMs")?
                            .as_i64()
                            .ok_or_else(unsupported)?,
                    )
                }
                WorkflowStepKind::WaitEvent => {
                    fields(&value, &["type", "timeoutMs"])?;
                    Self::WaitEvent {
                        event_type: required(&value, "type")?
                            .as_str()
                            .ok_or_else(unsupported)?
                            .into(),
                        timeout_ms: required(&value, "timeoutMs")?
                            .as_u64()
                            .ok_or_else(unsupported)?,
                    }
                }
            };
            if config.canonical_json()? != encoded {
                return Err(unsupported());
            }
            Ok(config)
        };
        decode().map_err(|_| error(ErrorCode::WorkflowInvariantViolation))
    }

    /// Validate a structured stored policy without changing its values.
    pub fn validate(&self) -> Result<(), PlatformError> {
        match self {
            Self::Do(config) => config.validate(),
            Self::Sleep(duration) if *duration <= WORKFLOW_MAX_DURATION_MS => Ok(()),
            Self::SleepUntil(timestamp)
                if timestamp.unsigned_abs() <= WORKFLOW_MAX_SAFE_INTEGER =>
            {
                Ok(())
            }
            Self::WaitEvent {
                event_type,
                timeout_ms,
            } if *timeout_ms <= WORKFLOW_MAX_DURATION_MS => {
                validate_workflow_event_type(event_type)
            }
            _ => Err(error(ErrorCode::WorkflowDurationInvalid)),
        }
    }

    /// Operation kind implied by this validated configuration.
    #[must_use]
    pub const fn kind(&self) -> WorkflowStepKind {
        match self {
            Self::Do(_) => WorkflowStepKind::Do,
            Self::Sleep(_) => WorkflowStepKind::Sleep,
            Self::SleepUntil(_) => WorkflowStepKind::SleepUntil,
            Self::WaitEvent { .. } => WorkflowStepKind::WaitEvent,
        }
    }

    /// Canonical resolved bytes persisted and compared during replay.
    pub fn canonical_json(&self) -> Result<String, PlatformError> {
        self.validate()?;
        let value = match self {
            Self::Do(config) => serde_json::to_value(config).map_err(|_| unsupported())?,
            Self::Sleep(duration) => json!({"durationMs":duration}),
            Self::SleepUntil(timestamp) => json!({"timestampMs":timestamp}),
            Self::WaitEvent {
                event_type,
                timeout_ms,
            } => json!({"type":event_type,"timeoutMs":timeout_ms}),
        };
        Ok(value.to_string())
    }
}

/// Strict private wire declaration before duration/default normalization.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkflowStepDeclaration {
    /// Zero-based API call order.
    pub ordinal: u32,
    /// Declared API operation.
    pub kind: WorkflowStepKind,
    /// UTF-8 display identity, bounded in bytes.
    pub name: String,
    /// One-based occurrence count for this kind and name.
    pub name_count: u32,
    /// Raw supported options, normalized at admission.
    pub config: Value,
    /// Rollback callback policy when the deployment registered a handler.
    #[serde(default)]
    pub rollback_config: Option<Value>,
    /// Whether this descriptor executes a previously registered rollback handler.
    #[serde(default)]
    pub rollback_step: bool,
    /// Ordered settled predecessor frontier for this submission group.
    pub dependencies: Vec<u32>,
    /// First ordinal in this durable submission group.
    pub batch_first_ordinal: u32,
    /// Complete submission-group membership count, never inferred from completion order.
    pub batch_size: u32,
}

impl WorkflowStepDeclaration {
    /// Resolve the supported policy and validate the complete replay declaration.
    pub fn resolve(self) -> Result<WorkflowStepDescriptor, PlatformError> {
        let rollback_config = self
            .rollback_config
            .map(|value| {
                if self.kind != WorkflowStepKind::Do {
                    return Err(unsupported());
                }
                WorkflowStepConfig::resolve(&value)
            })
            .transpose()?;
        if self.rollback_step && (self.kind != WorkflowStepKind::Do || rollback_config.is_some()) {
            return Err(unsupported());
        }
        let descriptor = WorkflowStepDescriptor {
            ordinal: self.ordinal,
            name: self.name,
            name_count: self.name_count,
            config: WorkflowDurableConfig::resolve(self.kind, &self.config)?,
            rollback_config,
            rollback_step: self.rollback_step,
            dependencies: self.dependencies,
            batch_first_ordinal: self.batch_first_ordinal,
            batch_size: self.batch_size,
        };
        descriptor.validate()?;
        Ok(descriptor)
    }
}

/// Canonical replay identity, including immutable batch and dependency shape.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkflowStepDescriptor {
    /// Zero-based API call order.
    pub ordinal: u32,
    /// UTF-8 display identity.
    pub name: String,
    /// One-based occurrence count for this kind and name.
    pub name_count: u32,
    /// Fully resolved frozen configuration.
    pub config: WorkflowDurableConfig,
    /// Frozen retry/timeout policy for an optional rollback handler.
    pub rollback_config: Option<WorkflowStepConfig>,
    /// Whether this is a scheduler-owned execution of a rollback handler.
    pub rollback_step: bool,
    /// Ordered predecessor ordinals; backend also compares the actual previous batch.
    pub dependencies: Vec<u32>,
    /// Immutable first batch ordinal.
    pub batch_first_ordinal: u32,
    /// Immutable complete batch size.
    pub batch_size: u32,
}

impl WorkflowStepDescriptor {
    /// Validate bounded names, ordinals, an acyclic predecessor set, and frozen policy.
    pub fn validate(&self) -> Result<(), PlatformError> {
        self.config.validate()?;
        if let Some(config) = &self.rollback_config {
            if self.config.kind() != WorkflowStepKind::Do || self.rollback_step {
                return Err(error(ErrorCode::WorkflowStepConfigUnsupported));
            }
            config.validate()?;
        }
        if self.rollback_step && self.config.kind() != WorkflowStepKind::Do {
            return Err(error(ErrorCode::WorkflowStepConfigUnsupported));
        }
        if self.ordinal >= 1024 {
            return Err(error(ErrorCode::WorkflowStepLimitExceeded));
        }
        if self.name.is_empty() || self.name.len() > 256 || self.name_count == 0 {
            return Err(error(ErrorCode::WorkflowSerializationUnsupported));
        }
        if self.batch_size == 0 || self.batch_size > 16 || self.dependencies.len() > 16 {
            return Err(error(ErrorCode::WorkflowStepLimitExceeded));
        }
        if self.batch_first_ordinal > self.ordinal
            || self
                .batch_first_ordinal
                .checked_add(self.batch_size)
                .is_none_or(|end| self.ordinal >= end || end > 1024)
            || self.dependencies.windows(2).any(|pair| pair[0] >= pair[1])
            || self
                .dependencies
                .last()
                .is_some_and(|parent| *parent >= self.batch_first_ordinal)
            || (self.batch_first_ordinal == 0 && !self.dependencies.is_empty())
        {
            return Err(error(ErrorCode::WorkflowSerializationUnsupported));
        }
        Ok(())
    }

    /// Digest the full normalized descriptor, never only the display name or config hash.
    pub fn sha256(&self) -> Result<[u8; 32], PlatformError> {
        self.validate()?;
        let config: Value =
            serde_json::from_str(&self.config.canonical_json()?).map_err(|_| unsupported())?;
        let rollback_config = self
            .rollback_config
            .as_ref()
            .map(serde_json::to_value)
            .transpose()
            .map_err(|_| unsupported())?;
        let descriptor = json!({"capabilityVersion":1,"ordinal":self.ordinal,"kind":self.config.kind(),"name":self.name,
            "nameCount":self.name_count,"config":config,"dependencies":self.dependencies,
            "batchFirstOrdinal":self.batch_first_ordinal,"batchSize":self.batch_size,
            "rollbackConfig":rollback_config,"rollbackStep":self.rollback_step});
        Ok(Sha256::digest(descriptor.to_string().as_bytes()).into())
    }

    /// Canonical persisted policy, including rollback registration identity.
    pub fn canonical_config_json(&self) -> Result<String, PlatformError> {
        self.validate()?;
        let encoded = self.config.canonical_json()?;
        let mut value: serde_json::Map<String, Value> =
            serde_json::from_str(&encoded).map_err(|_| unsupported())?;
        if let Some(config) = &self.rollback_config {
            value.insert(
                "rollbackConfig".into(),
                serde_json::to_value(config).map_err(|_| unsupported())?,
            );
        }
        value.insert("rollbackStep".into(), Value::Bool(self.rollback_step));
        Ok(Value::Object(value).to_string())
    }

    /// Exact retained descriptor/edge bytes, excluding later result and error bytes.
    pub fn state_bytes(&self) -> Result<usize, PlatformError> {
        self.validate()?;
        Ok(WORKFLOW_STEP_BYTES
            + self.name.len()
            + self.canonical_config_json()?.len()
            + WORKFLOW_DEPENDENCY_BYTES * self.dependencies.len())
    }
}

/// Validate the supported event alphabet without normalizing case or truncating input.
pub fn validate_workflow_event_type(value: &str) -> Result<(), PlatformError> {
    super::validate_workflow_instance_id(value)
        .map_err(|_| error(ErrorCode::WorkflowEventTypeInvalid))
}

fn unsupported() -> PlatformError {
    error(ErrorCode::WorkflowStepConfigUnsupported)
}
fn fields(value: &Value, expected: &[&str]) -> Result<(), PlatformError> {
    let object = value.as_object().ok_or_else(unsupported)?;
    if object.keys().any(|key| !expected.contains(&key.as_str())) {
        return Err(unsupported());
    }
    Ok(())
}
fn required<'a>(value: &'a Value, key: &str) -> Result<&'a Value, PlatformError> {
    value.get(key).ok_or_else(unsupported)
}

#[cfg(test)]
#[path = "descriptor_tests.rs"]
mod tests;
