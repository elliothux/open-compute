//! Canonical Workers observability model, filtering, redaction, and signed-ticket helpers.

use super::{
    DATASET, EffectiveIdentity, MAX_FILTER_TEXT, MAX_FILTER_VALUES, MAX_FILTERS, TailFilter,
    TailFrame,
};
use open_compute_core::{AccountId, ErrorCode, PlatformError, SecretString, VersionId, WorkerId};
use open_compute_storage::{NewObservabilityEvent, NewObservabilityInvocation, ObservabilityField};
use serde_json::{Map, Value, json};
use sha2::{Digest as _, Sha256};
use std::collections::{BTreeMap, HashSet};
use std::net::IpAddr;
use std::str::FromStr as _;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;

pub(super) fn canonical_invocation(
    collector_event_id: &str,
    index: usize,
    mut item: Value,
    identity: &EffectiveIdentity,
    received_at_ms: i64,
    batch_truncated: bool,
    max_invocation_log_bytes: u64,
) -> Result<NewObservabilityInvocation, PlatformError> {
    redact_trace_item(&mut item, &identity.secret_values);
    if u64::try_from(serde_json::to_vec(&item).map_err(|_| invalid())?.len())
        .map_err(|_| invalid())?
        > max_invocation_log_bytes
    {
        return Err(invalid());
    }
    let object = item.as_object_mut().ok_or_else(invalid)?;
    let event_timestamp_ms = object
        .get("eventTimestamp")
        .and_then(Value::as_i64)
        .unwrap_or(received_at_ms);
    if event_timestamp_ms < received_at_ms.saturating_sub(30 * 24 * 60 * 60 * 1_000)
        || event_timestamp_ms > received_at_ms.saturating_add(24 * 60 * 60 * 1_000)
    {
        return Err(invalid());
    }
    let outcome = bounded_string(object.get("outcome"), 64).ok_or_else(invalid)?;
    let cpu_time_ms = finite_number(object.get("cpuTime")).unwrap_or(0.0);
    let wall_time_ms = finite_number(object.get("wallTime")).unwrap_or(0.0);
    let event = object.get("event").cloned().unwrap_or(Value::Null);
    let event_type = event_type(&event);
    let invocation_id = opaque_id(&[
        collector_event_id,
        &index.to_string(),
        &identity.worker.id.to_string(),
        &identity.version_id.to_string(),
    ]);
    object.insert(
        "scriptName".to_owned(),
        Value::String(identity.worker.name.clone()),
    );
    object.insert(
        "scriptVersion".to_owned(),
        json!({ "id": identity.version_id.to_string() }),
    );
    let mut truncated = batch_truncated
        || object
            .get("truncated")
            .and_then(Value::as_bool)
            .unwrap_or(false);
    let logs = object
        .get("logs")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let exceptions = object
        .get("exceptions")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if logs.len() > 1_024 || exceptions.len() > 256 {
        return Err(invalid());
    }
    let mut events = Vec::new();
    if identity.settings.invocation_logs {
        let source = event.clone();
        events.push(public_event(
            &invocation_id,
            events.len(),
            event_timestamp_ms,
            "cf-worker-event",
            None,
            source,
            identity,
            &event_type,
            &outcome,
            cpu_time_ms,
            wall_time_ms,
            None,
        )?);
    }
    for log in logs {
        let Some(log) = log.as_object() else {
            truncated = true;
            continue;
        };
        let level = bounded_string(log.get("level"), 32).unwrap_or_else(|| "log".to_owned());
        let source = log_source(log.get("message"));
        let timestamp_ms = log
            .get("timestamp")
            .and_then(Value::as_i64)
            .unwrap_or(event_timestamp_ms);
        events.push(public_event(
            &invocation_id,
            events.len(),
            timestamp_ms,
            "cf-worker-log",
            Some(level),
            source,
            identity,
            &event_type,
            &outcome,
            cpu_time_ms,
            wall_time_ms,
            None,
        )?);
    }
    for exception in exceptions {
        let Some(exception) = exception.as_object() else {
            truncated = true;
            continue;
        };
        let message = bounded_string(exception.get("message"), 16_384)
            .unwrap_or_else(|| "Worker exception".to_owned());
        let timestamp_ms = exception
            .get("timestamp")
            .and_then(Value::as_i64)
            .unwrap_or(event_timestamp_ms);
        events.push(public_event(
            &invocation_id,
            events.len(),
            timestamp_ms,
            "cf-worker-log",
            Some("error".to_owned()),
            Value::String(message.clone()),
            identity,
            &event_type,
            &outcome,
            cpu_time_ms,
            wall_time_ms,
            Some(message),
        )?);
    }
    Ok(NewObservabilityInvocation {
        invocation_id,
        account_id: identity.account_id.to_string(),
        script_name: identity.worker.name.clone(),
        version_id: identity.version_id.to_string(),
        deployment_id: identity.deployment_id.clone(),
        event_timestamp_ms,
        received_at_ms,
        event_type,
        outcome,
        cpu_time_ms,
        wall_time_ms,
        truncated,
        event: item,
        events,
    })
}

#[allow(clippy::too_many_arguments)]
fn public_event(
    invocation_id: &str,
    sequence: usize,
    timestamp_ms: i64,
    metadata_type: &str,
    level: Option<String>,
    source: Value,
    identity: &EffectiveIdentity,
    event_type: &str,
    outcome: &str,
    cpu_time_ms: f64,
    wall_time_ms: f64,
    error: Option<String>,
) -> Result<NewObservabilityEvent, PlatformError> {
    let sequence = u32::try_from(sequence).map_err(|_| invalid())?;
    let event_id = format!("{invocation_id}:{sequence}");
    let mut metadata = Map::new();
    metadata.insert("id".to_owned(), Value::String(event_id.clone()));
    metadata.insert("type".to_owned(), Value::String(metadata_type.to_owned()));
    metadata.insert(
        "cloudService".to_owned(),
        Value::String("workers".to_owned()),
    );
    metadata.insert(
        "service".to_owned(),
        Value::String(identity.worker.name.clone()),
    );
    metadata.insert(
        "requestId".to_owned(),
        Value::String(invocation_id.to_owned()),
    );
    metadata.insert("origin".to_owned(), Value::String(event_type.to_owned()));
    metadata.insert("outcome".to_owned(), Value::String(outcome.to_owned()));
    metadata.insert("cpuTimeMs".to_owned(), json!(cpu_time_ms));
    metadata.insert("wallTimeMs".to_owned(), json!(wall_time_ms));
    if let Some(level) = &level {
        metadata.insert("level".to_owned(), Value::String(level.clone()));
        metadata.insert("message".to_owned(), Value::String(source_text(&source)));
    }
    if let Some(error) = error {
        metadata.insert("error".to_owned(), Value::String(error));
    }
    let metadata = Value::Object(metadata);
    let mut fields = BTreeMap::new();
    flatten_scalars("$metadata", &metadata, &mut fields, 0);
    fields.insert(
        "$workers.scriptName".to_owned(),
        Value::String(identity.worker.name.clone()),
    );
    fields.insert(
        "$workers.scriptVersion.id".to_owned(),
        Value::String(identity.version_id.to_string()),
    );
    fields.insert(
        "$workers.eventType".to_owned(),
        Value::String(event_type.to_owned()),
    );
    flatten_scalars("source", &source, &mut fields, 0);
    Ok(NewObservabilityEvent {
        event_id,
        sequence,
        timestamp_ms,
        metadata_type: metadata_type.to_owned(),
        level,
        source,
        metadata,
        fields: fields
            .into_iter()
            .take(256)
            .map(|(key, value)| ObservabilityField { key, value })
            .collect(),
    })
}

fn flatten_scalars(prefix: &str, value: &Value, output: &mut BTreeMap<String, Value>, depth: u8) {
    if output.len() >= 256 || depth >= 32 {
        return;
    }
    match value {
        Value::Bool(_) | Value::Number(_) | Value::String(_) => {
            if !prefix.is_empty() && prefix.len() <= 512 {
                output
                    .entry(prefix.to_owned())
                    .or_insert_with(|| value.clone());
            }
        }
        Value::Null => {}
        Value::Object(object) => {
            for (key, value) in object {
                if output.len() >= 256 {
                    break;
                }
                let next = if prefix.is_empty() {
                    key.clone()
                } else {
                    format!("{prefix}.{key}")
                };
                flatten_scalars(&next, value, output, depth + 1);
            }
        }
        Value::Array(_) => {}
    }
}

pub(super) fn live_event(
    invocation: &NewObservabilityInvocation,
    event: &NewObservabilityEvent,
) -> Value {
    let event_type = event
        .metadata
        .get("origin")
        .cloned()
        .unwrap_or(Value::String("unknown".to_owned()));
    json!({
        "source": event.source,
        "dataset": "",
        "timestamp": event.timestamp_ms,
        "$workers": {
            "scriptName": invocation.script_name,
            "scriptVersion": {"id": invocation.version_id},
            "eventType": event_type,
            "truncated": invocation.truncated,
            "requestId": invocation.invocation_id,
            "event": invocation.event.get("event").cloned().unwrap_or(Value::Null),
            "outcome": invocation.outcome,
            "cpuTimeMs": invocation.cpu_time_ms,
            "wallTimeMs": invocation.wall_time_ms
        },
        "$metadata": event.metadata
    })
}

pub(super) fn matches_tail(
    filters: &[TailFilter],
    invocation: &NewObservabilityInvocation,
    session_id: &str,
) -> bool {
    filters.iter().all(|filter| match filter {
        TailFilter::Sampling(rate) => sampled(&invocation.invocation_id, session_id, *rate),
        TailFilter::Outcome(values) => values.contains(&invocation.outcome),
        TailFilter::Method(values) => invocation
            .event
            .pointer("/event/request/method")
            .and_then(Value::as_str)
            .is_some_and(|method| {
                values
                    .iter()
                    .any(|value| value.eq_ignore_ascii_case(method))
            }),
        TailFilter::Header { key, query } => invocation
            .event
            .pointer("/event/request/headers")
            .and_then(Value::as_object)
            .and_then(|headers| {
                headers
                    .iter()
                    .find(|(name, _)| name.eq_ignore_ascii_case(key))
                    .and_then(|(_, value)| value.as_str())
            })
            .is_some_and(|value| value.contains(query)),
        TailFilter::ClientIp(values) => invocation
            .event
            .pointer("/event/request/headers")
            .and_then(Value::as_object)
            .and_then(|headers| {
                headers
                    .iter()
                    .find(|(name, _)| name.eq_ignore_ascii_case("cf-connecting-ip"))
                    .and_then(|(_, value)| value.as_str())
            })
            .and_then(|value| value.parse::<IpAddr>().ok())
            .is_some_and(|value| values.contains(&value)),
        TailFilter::Query(query) => invocation
            .event
            .get("logs")
            .and_then(Value::as_array)
            .is_some_and(|logs| {
                logs.iter()
                    .any(|log| source_text(&log["message"]).contains(query))
            }),
        TailFilter::ScriptVersion(version) => invocation.version_id == version.to_string(),
    })
}

pub(super) fn validate_filters(filters: &[TailFilter]) -> Result<(), PlatformError> {
    if filters.len() > MAX_FILTERS {
        return Err(invalid());
    }
    let mut kinds = HashSet::new();
    for filter in filters {
        let kind = match filter {
            TailFilter::Sampling(rate) if rate.is_finite() && *rate > 0.0 && *rate < 1.0 => 0,
            TailFilter::Outcome(values)
                if valid_list(values)
                    && values.iter().all(|value| {
                        matches!(
                            value.as_str(),
                            "ok" | "canceled"
                                | "exception"
                                | "exceededCpu"
                                | "exceededMemory"
                                | "unknown"
                        )
                    }) =>
            {
                1
            }
            TailFilter::Method(values)
                if valid_list(values)
                    && values.iter().all(|value| {
                        !value.is_empty()
                            && value.len() <= 32
                            && value.bytes().all(|byte| byte.is_ascii_alphabetic())
                    }) =>
            {
                2
            }
            TailFilter::Header { key, query }
                if !key.is_empty()
                    && key.len() <= 256
                    && !query.is_empty()
                    && query.len() <= MAX_FILTER_TEXT =>
            {
                3
            }
            TailFilter::ClientIp(values)
                if !values.is_empty() && values.len() <= MAX_FILTER_VALUES =>
            {
                4
            }
            TailFilter::Query(value) if !value.is_empty() && value.len() <= MAX_FILTER_TEXT => 5,
            TailFilter::ScriptVersion(_) => 6,
            _ => return Err(invalid()),
        };
        if !kinds.insert(kind) {
            return Err(invalid());
        }
    }
    Ok(())
}

fn valid_list(values: &[String]) -> bool {
    !values.is_empty() && values.len() <= MAX_FILTER_VALUES
}

pub(super) fn sampled(invocation_id: &str, namespace: &str, rate: f64) -> bool {
    if rate >= 1.0 {
        return true;
    }
    if rate <= 0.0 || !rate.is_finite() {
        return false;
    }
    let mut digest = Sha256::new();
    digest.update(namespace.as_bytes());
    digest.update([0]);
    digest.update(invocation_id.as_bytes());
    let bytes: [u8; 8] = digest.finalize()[..8].try_into().unwrap_or([0; 8]);
    (u64::from_be_bytes(bytes) as f64 / u64::MAX as f64) < rate
}

pub(super) fn loader_identity(value: &str) -> Option<(AccountId, WorkerId, VersionId)> {
    let mut parts = value.split('/');
    let account = AccountId::from_str(parts.next()?).ok()?;
    let worker = WorkerId::from_str(parts.next()?).ok()?;
    let version = VersionId::from_str(parts.next()?).ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some((account, worker, version))
}

fn event_type(event: &Value) -> String {
    let Some(object) = event.as_object() else {
        return "unknown".to_owned();
    };
    if object.contains_key("request") {
        "fetch".to_owned()
    } else if object.contains_key("cron") {
        "scheduled".to_owned()
    } else if object.contains_key("queue") {
        "queue".to_owned()
    } else if object.contains_key("mailFrom") {
        "email".to_owned()
    } else if object.contains_key("rpcMethod") {
        "rpc".to_owned()
    } else if object.contains_key("consumedEvents") {
        "tail".to_owned()
    } else if object.contains_key("scheduledTime") {
        "alarm".to_owned()
    } else if let Some(kind) = object.get("type").and_then(Value::as_str) {
        kind.chars().take(64).collect()
    } else {
        "unknown".to_owned()
    }
}

fn log_source(value: Option<&Value>) -> Value {
    let Some(value) = value else {
        return Value::String(String::new());
    };
    let values = value
        .as_array()
        .map_or(std::slice::from_ref(value), Vec::as_slice);
    if values.len() == 1 && values[0].is_object() {
        values[0].clone()
    } else {
        Value::String(values.iter().map(source_text).collect::<Vec<_>>().join(" "))
    }
}

fn source_text(value: &Value) -> String {
    match value {
        Value::String(value) => value.clone(),
        Value::Null => "null".to_owned(),
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        Value::Array(_) | Value::Object(_) => serde_json::to_string(value).unwrap_or_default(),
    }
}

fn finite_number(value: Option<&Value>) -> Option<f64> {
    value
        .and_then(Value::as_f64)
        .filter(|value| value.is_finite() && *value >= 0.0)
}

fn bounded_string(value: Option<&Value>, maximum: usize) -> Option<String> {
    value
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty() && value.len() <= maximum)
        .map(ToOwned::to_owned)
}

fn opaque_id(parts: &[&str]) -> String {
    let mut digest = Sha256::new();
    for part in parts {
        digest.update(part.as_bytes());
        digest.update([0]);
    }
    hex::encode(digest.finalize())
}

pub(super) fn ticket_claim(
    id: &str,
    account_id: AccountId,
    worker_id: WorkerId,
    expires_at_ms: i64,
) -> String {
    format!("v1\0{id}\0{account_id}\0{worker_id}\0{expires_at_ms}")
}

pub(super) fn enqueue_overload(
    sender: &mpsc::Sender<TailFrame>,
    queued: &Arc<AtomicUsize>,
    start: bool,
    live: bool,
) {
    let message = if start {
        "Tail events are being dropped because the client is too slow."
    } else {
        "Tail event delivery has resumed."
    };
    let event = if live {
        json!({
            "source": {"level": "warn", "message": message},
            "dataset": "",
            "timestamp": now_ms().unwrap_or(0),
            "$workers": {"scriptName": "open-compute", "eventType": "tail", "truncated": false},
            "$metadata": {"type": "cf-worker-log", "origin": "tail", "message": message}
        })
    } else {
        json!({
            "outcome": "ok",
            "scriptName": "open-compute",
            "exceptions": [],
            "logs": [],
            "eventTimestamp": now_ms().unwrap_or(0),
            "event": {
                "type": if start { "overload" } else { "overload-stop" },
                "message": message
            }
        })
    };
    let Ok(text) = serde_json::to_string(&event) else {
        return;
    };
    let bytes = text.len();
    queued.fetch_add(bytes, Ordering::Relaxed);
    let _ = sender.try_send(TailFrame {
        text,
        bytes,
        queued_bytes: queued.clone(),
    });
}

fn redact_trace_item(item: &mut Value, secrets: &[SecretString]) {
    redact_secret_values(item, secrets);
    let Some(request) = item
        .get_mut("event")
        .and_then(Value::as_object_mut)
        .and_then(|event| event.get_mut("request"))
        .and_then(Value::as_object_mut)
    else {
        return;
    };
    if let Some(url) = request.get_mut("url")
        && let Some(raw) = url.as_str()
    {
        *url = Value::String(redacted_url(raw));
    }
    let Some(headers) = request.get_mut("headers").and_then(Value::as_object_mut) else {
        return;
    };
    headers.retain(|name, value| {
        if name.to_ascii_lowercase().starts_with("x-open-compute-") {
            return false;
        }
        if secret_header(name) || !value.is_string() {
            *value = Value::String("REDACTED".to_owned());
        }
        true
    });
}

fn redact_secret_values(value: &mut Value, secrets: &[SecretString]) {
    match value {
        Value::String(text) => redact_secret_text(text, secrets),
        Value::Array(values) => {
            for value in values {
                redact_secret_values(value, secrets);
            }
        }
        Value::Object(object) => {
            let mut redacted = Map::new();
            for (mut key, mut value) in std::mem::take(object) {
                redact_secret_text(&mut key, secrets);
                redact_secret_values(&mut value, secrets);
                redacted.insert(key, value);
            }
            *object = redacted;
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
}

fn redact_secret_text(value: &mut String, secrets: &[SecretString]) {
    for secret in secrets {
        let exposed = secret.expose();
        if value.contains(exposed) {
            *value = value.replace(exposed, "REDACTED");
        }
    }
}

fn secret_header(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower == "cookie"
        || lower == "set-cookie"
        || ["auth", "key", "secret", "token", "jwt"]
            .iter()
            .any(|part| lower.contains(part))
}

fn redacted_url(raw: &str) -> String {
    let Ok(mut url) = url::Url::parse(raw) else {
        return "https://redacted.invalid/".to_owned();
    };
    let pairs = url
        .query_pairs()
        .map(|(key, value)| {
            let key = key.into_owned();
            let value = if secret_header(&key) {
                "REDACTED".to_owned()
            } else {
                value.into_owned()
            };
            (key, value)
        })
        .collect::<Vec<_>>();
    url.set_query(None);
    if !pairs.is_empty() {
        let mut query = url.query_pairs_mut();
        for (key, value) in pairs {
            query.append_pair(&key, &value);
        }
    }
    url.set_username("").ok();
    url.set_password(None).ok();
    let path = url
        .path_segments()
        .map(|parts| {
            parts
                .map(|part| {
                    if part.len() >= 24
                        && part
                            .bytes()
                            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
                    {
                        "REDACTED".to_owned()
                    } else {
                        part.to_owned()
                    }
                })
                .collect::<Vec<_>>()
                .join("/")
        })
        .unwrap_or_default();
    url.set_path(&format!("/{path}"));
    url.into()
}

pub(super) fn constant_time_equal(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    use subtle::ConstantTimeEq as _;
    bool::from(left.ct_eq(right))
}

pub(super) fn format_timestamp(value: i64) -> Result<String, PlatformError> {
    jiff::Timestamp::from_millisecond(value)
        .map(|value| value.to_string())
        .map_err(|_| invalid())
}

pub(super) fn now_ms() -> Result<i64, PlatformError> {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| unavailable())?
        .as_millis();
    i64::try_from(millis).map_err(|_| unavailable())
}

pub(super) fn invalid() -> PlatformError {
    PlatformError::new(ErrorCode::LimitInvalid, "observability request is invalid")
}

pub(super) fn stale() -> PlatformError {
    PlatformError::new(
        ErrorCode::VersionInvariantViolation,
        "observability identity is stale or invalid",
    )
}

pub(super) fn not_found() -> PlatformError {
    PlatformError::new(
        ErrorCode::ResourceNotFound,
        "Script Tail session was not found",
    )
}

pub(super) fn unavailable() -> PlatformError {
    PlatformError::new(
        ErrorCode::PlatformUnavailable,
        "observability service is unavailable",
    )
}

/// Cloudflare Workers Logs dataset identifier.
pub(crate) const fn workers_logs_dataset() -> &'static str {
    DATASET
}

#[cfg(test)]
#[path = "observability_model_tests.rs"]
mod tests;
