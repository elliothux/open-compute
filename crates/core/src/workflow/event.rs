//! Durable event envelopes preserve the payload's independent JSON size and depth limits.

use super::{
    WORKFLOW_JSON_MAX_BYTES, WORKFLOW_MAX_SAFE_INTEGER, canonical_json, error,
    validate_workflow_event_type,
};
use crate::{ErrorCode, PlatformError};
use serde::Deserialize;
use serde_json::value::RawValue;

/// Maximum retained event result: one full payload plus bounded trusted envelope metadata.
pub const WORKFLOW_EVENT_ENVELOPE_MAX_BYTES: usize = WORKFLOW_JSON_MAX_BYTES + 1024;

/// Validated event result copied atomically from the inbox before consumption.
/// Payload bytes retain their own depth/size budget, independent of this outer object.
pub struct WorkflowEventEnvelope<'a> {
    /// Exact admitted ASCII event type.
    pub event_type: &'a str,
    /// Canonical payload, never reinterpreted using an arbitrary precision number parser.
    pub payload_json: &'a str,
    /// Authority timestamp of the original event admission.
    pub timestamp_ms: i64,
}

impl std::fmt::Debug for WorkflowEventEnvelope<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkflowEventEnvelope")
            .field("payload_bytes", &self.payload_json.len())
            .field("timestamp_ms", &self.timestamp_ms)
            .finish_non_exhaustive()
    }
}

impl WorkflowEventEnvelope<'_> {
    /// Encode already canonical input; invalid persisted payloads are rejected, not repaired.
    pub fn canonical_json(&self) -> Result<String, PlatformError> {
        validate_workflow_event_type(self.event_type)?;
        if self.timestamp_ms.unsigned_abs() > WORKFLOW_MAX_SAFE_INTEGER {
            return Err(error(ErrorCode::WorkflowDurationInvalid));
        }
        if canonical_json(self.payload_json, ErrorCode::WorkflowPayloadTooLarge)?
            != self.payload_json
        {
            return Err(error(ErrorCode::WorkflowInvariantViolation));
        }
        // The type alphabet excludes all characters that need JSON escaping.
        let output = format!(
            r#"{{"payload":{},"timestampMs":{},"type":"{}"}}"#,
            self.payload_json, self.timestamp_ms, self.event_type
        );
        if output.len() > WORKFLOW_EVENT_ENVELOPE_MAX_BYTES {
            return Err(error(ErrorCode::WorkflowResultTooLarge));
        }
        Ok(output)
    }

    /// Read an immutable stored envelope without applying payload limits to the outer object.
    /// Exact fields, spelling and canonical bytes are checked before replay.
    pub fn from_canonical(encoded: &str) -> Result<WorkflowEventEnvelope<'_>, PlatformError> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        struct Envelope<'a> {
            #[serde(rename = "type", borrow)]
            event_type: &'a str,
            #[serde(borrow)]
            payload: &'a RawValue,
            timestamp_ms: i64,
        }
        let decode = || {
            if encoded.len() > WORKFLOW_EVENT_ENVELOPE_MAX_BYTES {
                return Err(error(ErrorCode::WorkflowResultTooLarge));
            }
            let value: Envelope<'_> = serde_json::from_str(encoded)
                .map_err(|_| error(ErrorCode::WorkflowInvariantViolation))?;
            let envelope = WorkflowEventEnvelope {
                event_type: value.event_type,
                payload_json: value.payload.get(),
                timestamp_ms: value.timestamp_ms,
            };
            if envelope.canonical_json()? != encoded {
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
    fn envelope_does_not_reduce_payload_size_or_depth() {
        for payload in [
            format!("\"{}\"", "a".repeat(WORKFLOW_JSON_MAX_BYTES - 2)),
            format!("{}null{}", "[".repeat(127), "]".repeat(127)),
            "9007199254740992".into(),
        ] {
            let event = WorkflowEventEnvelope {
                event_type: "accepted",
                payload_json: &payload,
                timestamp_ms: 42,
            };
            let encoded = event.canonical_json().unwrap();
            let decoded = WorkflowEventEnvelope::from_canonical(&encoded).unwrap();
            assert_eq!(decoded.event_type, event.event_type);
            assert_eq!(decoded.payload_json, payload);
            assert_eq!(decoded.timestamp_ms, 42);
        }
    }

    #[test]
    fn immutable_envelope_rejects_corruption_without_normalization() {
        for invalid in [
            r#"{"type":"ok","payload":null,"timestampMs":1}"#,
            r#"{"payload":null,"timestampMs":1,"type":"ok","extra":0}"#,
            r#"{"payload":null,"timestampMs":1,"type":"ok","type":"ok"}"#,
            r#"{"payload":9007199254740993,"timestampMs":1,"type":"ok"}"#,
            r#"{"payload":null,"timestampMs":9007199254740992,"type":"ok"}"#,
            r#"{"payload":null,"timestampMs":1.5,"type":"ok"}"#,
            r#"{"payload":null,"timestampMs":1,"type":"-invalid"}"#,
        ] {
            assert_eq!(
                WorkflowEventEnvelope::from_canonical(invalid)
                    .err()
                    .unwrap()
                    .code(),
                ErrorCode::WorkflowInvariantViolation
            );
        }
        for payload in [
            format!("\"{}\"", "a".repeat(WORKFLOW_JSON_MAX_BYTES - 1)),
            format!("{}null{}", "[".repeat(128), "]".repeat(128)),
        ] {
            let encoded = format!(r#"{{"payload":{payload},"timestampMs":1,"type":"ok"}}"#);
            assert!(WorkflowEventEnvelope::from_canonical(&encoded).is_err());
        }
    }
}
