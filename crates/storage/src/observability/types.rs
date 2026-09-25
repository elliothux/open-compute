use serde::Serialize;
use serde_json::Value;

/// One discovered telemetry field.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ObservabilityFieldKey {
    /// Canonical dotted key.
    pub key: String,
    /// Scalar value type.
    #[serde(rename = "type")]
    pub value_type: String,
    /// Most recent event timestamp containing this key.
    pub last_seen_at: i64,
}

/// One bounded distinct value for a telemetry field.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ObservabilityFieldValue {
    /// Scalar value type.
    #[serde(rename = "type")]
    pub value_type: String,
    /// Scalar value.
    pub value: Value,
}

/// One per-service, per-day Workers Logs usage bucket.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ObservabilityUsageBreakdown {
    /// UTC day boundary formatted like Cloudflare's usage response.
    pub bin: String,
    /// Cloudflare-compatible dataset name.
    pub dataset: &'static str,
    /// External Worker Script name.
    pub service: String,
    /// Persisted public events in this bucket.
    pub count: u64,
}

/// Account-wide Workers Logs usage in one half-open time range.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ObservabilityUsage {
    /// Total persisted public events in the range.
    pub events: u64,
    /// Per-service daily buckets.
    pub breakdown: Vec<ObservabilityUsageBreakdown>,
}
