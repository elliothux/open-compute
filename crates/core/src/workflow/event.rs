//! Durable event metadata around an independently encoded structured-clone payload.

use super::{
    WORKFLOW_MAX_SAFE_INTEGER, WORKFLOW_VALUE_MAX_BYTES, durable_value_base64, error,
    validate_workflow_event_type,
};
use crate::{ErrorCode, PlatformError};
use serde::{Deserialize, Serialize};

/// Immutable metadata for an instance created by a direct Workflow cron schedule.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkflowCronSchedule {
    /// Exact cron expression that triggered the instance.
    pub cron: String,
    /// Logical UTC slot in Unix milliseconds.
    pub scheduled_time: i64,
}

impl WorkflowCronSchedule {
    /// Validate the exact expression and JavaScript-safe logical timestamp.
    pub fn validate(&self) -> Result<(), PlatformError> {
        crate::CronSchedule::parse(&self.cron)?;
        if self.scheduled_time < 0
            || self.scheduled_time % 60_000 != 0
            || self.scheduled_time.unsigned_abs() > WORKFLOW_MAX_SAFE_INTEGER
        {
            return Err(error(ErrorCode::WorkflowDurationInvalid));
        }
        Ok(())
    }
}

/// Maximum retained event result: one full payload plus bounded trusted metadata.
pub const WORKFLOW_EVENT_ENVELOPE_MAX_BYTES: usize = WORKFLOW_VALUE_MAX_BYTES * 4 / 3 + 2048;

/// Validated event result copied atomically from the inbox before consumption.
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkflowEventEnvelope<'a> {
    /// Exact admitted ASCII event type.
    #[serde(rename = "type")]
    pub event_type: &'a str,
    /// Canonical standard-base64 durable-value payload.
    pub payload_base64: &'a str,
    /// Authority timestamp of the original event admission.
    pub timestamp_ms: i64,
}

impl std::fmt::Debug for WorkflowEventEnvelope<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkflowEventEnvelope")
            .field("payload_bytes", &self.payload_base64.len())
            .field("timestamp_ms", &self.timestamp_ms)
            .finish_non_exhaustive()
    }
}

impl WorkflowEventEnvelope<'_> {
    /// Encode trusted metadata around an already validated durable-value payload.
    pub fn canonical_wire(&self) -> Result<String, PlatformError> {
        validate_workflow_event_type(self.event_type)?;
        if self.timestamp_ms.unsigned_abs() > WORKFLOW_MAX_SAFE_INTEGER {
            return Err(error(ErrorCode::WorkflowDurationInvalid));
        }
        durable_value_base64(self.payload_base64, ErrorCode::WorkflowPayloadTooLarge)?;
        let output = serde_json::to_string(self)
            .map_err(|_| error(ErrorCode::WorkflowInvariantViolation))?;
        if output.len() > WORKFLOW_EVENT_ENVELOPE_MAX_BYTES {
            return Err(error(ErrorCode::WorkflowResultTooLarge));
        }
        Ok(output)
    }

    /// Read immutable stored metadata without decoding the tenant value as JSON.
    pub fn from_wire(encoded: &'_ str) -> Result<WorkflowEventEnvelope<'_>, PlatformError> {
        let decode = || {
            if encoded.len() > WORKFLOW_EVENT_ENVELOPE_MAX_BYTES {
                return Err(error(ErrorCode::WorkflowResultTooLarge));
            }
            let envelope: WorkflowEventEnvelope<'_> = serde_json::from_str(encoded)
                .map_err(|_| error(ErrorCode::WorkflowInvariantViolation))?;
            if envelope.canonical_wire()? != encoded {
                return Err(error(ErrorCode::WorkflowInvariantViolation));
            }
            Ok(envelope)
        };
        decode().map_err(|_| error(ErrorCode::WorkflowInvariantViolation))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_wire_preserves_structured_clone_bytes() {
        let event = WorkflowEventEnvelope {
            event_type: "accepted",
            payload_base64: "T0NEVgECDA==",
            timestamp_ms: 42,
        };
        let encoded = event.canonical_wire().unwrap();
        let decoded = WorkflowEventEnvelope::from_wire(&encoded).unwrap();
        assert_eq!(decoded.event_type, event.event_type);
        assert_eq!(decoded.payload_base64, event.payload_base64);
        assert_eq!(decoded.timestamp_ms, 42);
        assert_eq!(
            format!("{event:?}"),
            "WorkflowEventEnvelope { payload_bytes: 12, timestamp_ms: 42, .. }"
        );
    }

    #[test]
    fn event_wire_rejects_invalid_or_noncanonical_metadata() {
        for event in [
            WorkflowEventEnvelope {
                event_type: "",
                payload_base64: "T0NEVgECDA==",
                timestamp_ms: 42,
            },
            WorkflowEventEnvelope {
                event_type: "accepted",
                payload_base64: "not-base64",
                timestamp_ms: 42,
            },
            WorkflowEventEnvelope {
                event_type: "accepted",
                payload_base64: "T0NEVgECDA==",
                timestamp_ms: i64::MAX,
            },
        ] {
            assert!(event.canonical_wire().is_err());
        }
        let canonical = WorkflowEventEnvelope {
            event_type: "accepted",
            payload_base64: "T0NEVgECDA==",
            timestamp_ms: 42,
        }
        .canonical_wire()
        .unwrap();
        assert!(WorkflowEventEnvelope::from_wire(&format!(" {canonical}")).is_err());
        assert!(WorkflowEventEnvelope::from_wire("not-json").is_err());
        assert!(
            WorkflowEventEnvelope::from_wire(&"x".repeat(WORKFLOW_EVENT_ENVELOPE_MAX_BYTES + 1))
                .is_err()
        );
    }
}
