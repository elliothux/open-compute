//! Frozen capability V2 retry, timeout, and instance retention policy.

use super::{WORKFLOW_MAX_DURATION_MS, WORKFLOW_MAX_SAFE_INTEGER, duration_ms, error};
use crate::{ErrorCode, PlatformError};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// Trusted drain budget reserved at the end of every activation.
pub const WORKFLOW_DRAIN_MARGIN_MS: u64 = 30_000;
/// Maximum duration of one callback attempt.
pub const WORKFLOW_MAX_ATTEMPT_MS: u64 = 240_000;
/// Saturation cap for a computed retry delay, independent of the base delay.
pub const WORKFLOW_MAX_RETRY_DELAY_MS: u64 = 86_400_000;

/// Static deterministic retry backoff, without dynamic functions or jitter.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum WorkflowBackoff {
    /// Use the frozen base delay after every failed attempt.
    Constant,
    /// Multiply the base delay by the one-based failed attempt number.
    Linear,
    /// Double the base delay for each subsequent failed attempt.
    #[default]
    Exponential,
}

/// Fully resolved retry policy frozen into a durable step descriptor.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WorkflowRetryPolicy {
    /// Number of additional business attempts, from zero through one hundred.
    pub limit: u32,
    /// Normalized base delay in milliseconds, before the computed-delay cap.
    pub delay: u64,
    /// Frozen deterministic backoff formula.
    pub backoff: WorkflowBackoff,
}

impl Default for WorkflowRetryPolicy {
    fn default() -> Self {
        Self {
            limit: 5,
            delay: 10_000,
            backoff: WorkflowBackoff::Exponential,
        }
    }
}

impl WorkflowRetryPolicy {
    /// Calculate the durable delay after a one-based business attempt.
    /// Overflow saturates at the declared daily cap rather than wrapping.
    pub fn delay_after(&self, attempt: u32) -> Result<u64, PlatformError> {
        self.validate()?;
        if !(1..=101).contains(&attempt) {
            return Err(unsupported());
        }
        let factor = match self.backoff {
            WorkflowBackoff::Constant => 1,
            WorkflowBackoff::Linear => u64::from(attempt),
            WorkflowBackoff::Exponential => 1_u64.checked_shl(attempt - 1).unwrap_or(u64::MAX),
        };
        Ok(self
            .delay
            .saturating_mul(factor)
            .min(WORKFLOW_MAX_RETRY_DELAY_MS))
    }

    fn validate(&self) -> Result<(), PlatformError> {
        if self.limit > 100 || self.delay > WORKFLOW_MAX_DURATION_MS {
            return Err(unsupported());
        }
        Ok(())
    }
}

/// Fully resolved `step.do` configuration, identical to the public step context.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WorkflowStepConfig {
    /// Frozen retry policy; operator capacity changes do not alter it.
    pub retries: WorkflowRetryPolicy,
    /// Per-attempt timeout in milliseconds, excluding durable waiting.
    pub timeout: u64,
}

impl Default for WorkflowStepConfig {
    fn default() -> Self {
        Self {
            retries: WorkflowRetryPolicy::default(),
            timeout: 60_000,
        }
    }
}

impl WorkflowStepConfig {
    /// Strictly normalize a public config object once at the authority boundary.
    pub fn resolve(value: &Value) -> Result<Self, PlatformError> {
        let fields = object(value, &["retries", "timeout"])?;
        let mut resolved = Self::default();
        if let Some(timeout) = fields.get("timeout") {
            resolved.timeout = duration_ms(timeout, WORKFLOW_MAX_ATTEMPT_MS)?;
        }
        if let Some(retries) = fields.get("retries") {
            let retries = object(retries, &["limit", "delay", "backoff"])?;
            resolved.retries.limit = retries
                .get("limit")
                .and_then(Value::as_f64)
                .filter(|value| value.fract() == 0.0 && (0.0..=100.0).contains(value))
                .map(|value| value as u32)
                .ok_or_else(unsupported)?;
            resolved.retries.delay = duration_ms(
                retries.get("delay").ok_or_else(unsupported)?,
                WORKFLOW_MAX_DURATION_MS,
            )?;
            if let Some(backoff) = retries.get("backoff") {
                resolved.retries.backoff =
                    serde_json::from_value(backoff.clone()).map_err(|_| unsupported())?;
            }
        }
        resolved.validate()?;
        Ok(resolved)
    }

    /// Verify a fully resolved stored config without repairing or changing defaults.
    pub fn validate(&self) -> Result<(), PlatformError> {
        self.retries.validate()?;
        if !(1..=WORKFLOW_MAX_ATTEMPT_MS).contains(&self.timeout) {
            return Err(unsupported());
        }
        Ok(())
    }

    /// Canonical descriptor bytes, with deterministic lexical object-key ordering.
    pub fn canonical_json(&self) -> Result<String, PlatformError> {
        self.validate()?;
        serde_json::to_value(self)
            .map(|value| value.to_string())
            .map_err(|_| error(ErrorCode::WorkflowInvariantViolation))
    }

    /// Whether an activation has enough remaining monotonic budget to start an attempt.
    #[must_use]
    pub fn fits_activation(&self, remaining_ms: u64) -> bool {
        self.timeout
            .checked_add(WORKFLOW_DRAIN_MARGIN_MS)
            .is_some_and(|required| required <= remaining_ms)
    }
}

/// Success and failure retention durations frozen at instance creation.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkflowRetention {
    /// Retention after successful completion, in milliseconds.
    pub success_retention_ms: u64,
    /// Retention after an error or termination, in milliseconds.
    pub error_retention_ms: u64,
}

impl Default for WorkflowRetention {
    fn default() -> Self {
        Self {
            success_retention_ms: 7 * 86_400_000,
            error_retention_ms: 30 * 86_400_000,
        }
    }
}

impl WorkflowRetention {
    /// Normalize public overrides against current creation defaults; stored policies never call this.
    pub fn resolve(value: &Value, defaults: &Self) -> Result<Self, PlatformError> {
        defaults.validate()?;
        let fields = object(value, &["successRetention", "errorRetention"])?;
        let mut retention = defaults.clone();
        if let Some(value) = fields.get("successRetention") {
            retention.success_retention_ms = duration_ms(value, WORKFLOW_MAX_DURATION_MS)?;
        }
        if let Some(value) = fields.get("errorRetention") {
            retention.error_retention_ms = duration_ms(value, WORKFLOW_MAX_DURATION_MS)?;
        }
        retention.validate()?;
        Ok(retention)
    }

    /// Check stored retention without adopting current operator policy.
    pub fn validate(&self) -> Result<(), PlatformError> {
        if [self.success_retention_ms, self.error_retention_ms]
            .into_iter()
            .any(|duration| !(3_600_000..=WORKFLOW_MAX_DURATION_MS).contains(&duration))
        {
            return Err(error(ErrorCode::WorkflowDurationInvalid));
        }
        Ok(())
    }

    /// Freeze an absolute expiry at the terminal transition; reads never extend it.
    pub fn expires_at(&self, terminal_at_ms: i64, success: bool) -> Result<i64, PlatformError> {
        self.validate()?;
        if terminal_at_ms.unsigned_abs() > WORKFLOW_MAX_SAFE_INTEGER {
            return Err(error(ErrorCode::WorkflowDurationInvalid));
        }
        let duration = if success {
            self.success_retention_ms
        } else {
            self.error_retention_ms
        };
        terminal_at_ms
            .checked_add(
                i64::try_from(duration).map_err(|_| error(ErrorCode::WorkflowDurationInvalid))?,
            )
            .filter(|expiry| expiry.unsigned_abs() <= WORKFLOW_MAX_SAFE_INTEGER)
            .ok_or_else(|| error(ErrorCode::WorkflowDurationInvalid))
    }
}

fn unsupported() -> PlatformError {
    error(ErrorCode::WorkflowStepConfigUnsupported)
}

fn object<'a>(value: &'a Value, fields: &[&str]) -> Result<&'a Map<String, Value>, PlatformError> {
    let object = value.as_object().ok_or_else(unsupported)?;
    if object.keys().any(|field| !fields.contains(&field.as_str())) {
        return Err(unsupported());
    }
    Ok(object)
}
