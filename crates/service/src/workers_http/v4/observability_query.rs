//! Cloudflare Workers Observability Telemetry query adapters.

use super::{domain, handlers};
use crate::cloudflare_v4::{V4Error, V4Permission, error_response};
use crate::http::HttpState;
use crate::observability::{ObservabilityService, workers_logs_dataset};
use crate::observability_filter::{
    Combination, FilterNode, ScalarType, collect_keys as collect_filter_keys, field_value,
    flatten_public, matches as filter_matches, scalar_kind, scalar_type,
    validate as validate_filter_ast,
};
use axum::extract::{Path, Request, State};
use axum::response::Response;
use open_compute_core::{AccountId, ErrorCode, PlatformError, RequestId};
use open_compute_storage::{
    ObservabilityAudit, ObservabilityEventCursor, ObservabilityFieldKey, ObservabilityFieldValue,
    StoredObservabilityEvent,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const QUERY_CANDIDATES: u32 = 20_000;
const QUERY_RESULT_MAX_BYTES: usize = 8 * 1024 * 1024;
const QUERY_DEADLINE: Duration = Duration::from_secs(2);

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Timeframe {
    from: i64,
    to: i64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct KeysBody {
    #[serde(default)]
    datasets: Vec<String>,
    #[serde(default)]
    filters: Vec<FilterNode>,
    from: Option<i64>,
    to: Option<i64>,
    limit: Option<u32>,
    key_needle: Option<Value>,
    needle: Option<Value>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ValuesBody {
    datasets: Vec<String>,
    key: String,
    timeframe: Timeframe,
    #[serde(rename = "type")]
    value_type: ScalarType,
    #[serde(default)]
    filters: Vec<FilterNode>,
    limit: Option<u32>,
    needle: Option<Value>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct QueryParameters {
    #[serde(default)]
    datasets: Vec<String>,
    #[serde(default)]
    filter_combination: Combination,
    #[serde(default)]
    filters: Vec<FilterNode>,
    limit: Option<u32>,
    calculations: Option<Value>,
    group_bys: Option<Value>,
    havings: Option<Value>,
    needle: Option<Value>,
    order_by: Option<Value>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
enum QueryView {
    Events,
    Invocations,
    Traces,
    Calculations,
    Requests,
    Agents,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct QueryBody {
    query_id: String,
    timeframe: Timeframe,
    parameters: Option<QueryParameters>,
    #[serde(default = "default_view")]
    view: QueryView,
    limit: Option<u32>,
    offset: Option<String>,
    #[serde(default)]
    dry: bool,
    chart: Option<bool>,
    chart_type: Option<String>,
    compare: Option<bool>,
    granularity: Option<u32>,
    ignore_series: Option<bool>,
    offset_by: Option<u32>,
    offset_direction: Option<String>,
}

fn default_view() -> QueryView {
    QueryView::Events
}

pub(super) async fn telemetry_keys(
    State(state): State<HttpState>,
    Path(account): Path<String>,
    request: Request,
) -> Response {
    let context = match handlers::authorize(&request, V4Permission::Read) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let body: KeysBody = match super::super::json::json_body(request).await {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    let started = Instant::now();
    let result = (|| {
        if body.key_needle.is_some() || body.needle.is_some() {
            return Err(V4Error::Unsupported);
        }
        validate_datasets(&body.datasets)?;
        validate_filters(&body.filters)?;
        let account_id = domain::resolve_account(&state, &account)?;
        let service = observability(&state)?;
        let now = now_ms().map_err(|error| V4Error::from(&error))?;
        let default_window = service.config().retention_ms.min(24 * 60 * 60 * 1_000);
        let from = body
            .from
            .unwrap_or(now.saturating_sub(i64::try_from(default_window).unwrap_or(i64::MAX)));
        let to = body.to.unwrap_or(now);
        validate_timeframe(service, from, to)?;
        let limit = body.limit.unwrap_or(1_000).clamp(1, 10_000);
        if body.filters.is_empty() {
            let values = service
                .store()
                .ok_or(V4Error::Unavailable)?
                .keys(&account_id.to_string(), from, to, limit)
                .map_err(|error| V4Error::from(&error))?;
            return bounded_result(values);
        }
        let events = candidates(service, account_id, from, to, None)?;
        let mut keys = BTreeMap::<(String, String), i64>::new();
        for (index, event) in events.into_iter().enumerate() {
            check_query_deadline(started, index)?;
            let public = public_event(&event, None);
            if matches_nodes(&body.filters, Combination::And, &public)? {
                let mut values = BTreeMap::new();
                flatten_public("", &public, &mut values, 0);
                for (key, value) in values {
                    if let Some(kind) = scalar_kind(&value) {
                        keys.entry((key, kind.to_owned()))
                            .and_modify(|seen| *seen = (*seen).max(event.timestamp_ms))
                            .or_insert(event.timestamp_ms);
                    }
                }
            }
        }
        bounded_result(
            keys.into_iter()
                .take(limit as usize)
                .map(|((key, value_type), last_seen_at)| ObservabilityFieldKey {
                    key,
                    value_type,
                    last_seen_at,
                })
                .collect::<Vec<_>>(),
        )
    })();
    handlers::respond(context, result)
}

pub(super) async fn telemetry_values(
    State(state): State<HttpState>,
    Path(account): Path<String>,
    request: Request,
) -> Response {
    let context = match handlers::authorize(&request, V4Permission::Read) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let body: ValuesBody = match super::super::json::json_body(request).await {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    let started = Instant::now();
    let result = (|| {
        if body.needle.is_some() {
            return Err(V4Error::Unsupported);
        }
        validate_datasets(&body.datasets)?;
        validate_filters(&body.filters)?;
        if body.key.is_empty() || body.key.len() > 512 {
            return Err(V4Error::InvalidField("/key"));
        }
        let account_id = domain::resolve_account(&state, &account)?;
        let service = observability(&state)?;
        validate_timeframe(service, body.timeframe.from, body.timeframe.to)?;
        let limit = body.limit.unwrap_or(100).clamp(1, 2_000);
        if body.filters.is_empty() {
            let values = service
                .store()
                .ok_or(V4Error::Unavailable)?
                .values(
                    &account_id.to_string(),
                    &body.key,
                    body.value_type.as_str(),
                    body.timeframe.from,
                    body.timeframe.to,
                    limit,
                )
                .map_err(|error| V4Error::from(&error))?;
            return bounded_result(
                values
                    .into_iter()
                    .map(|value| value_response(&body.key, &value))
                    .collect::<Vec<_>>(),
            );
        }
        let events = candidates(
            service,
            account_id,
            body.timeframe.from,
            body.timeframe.to,
            None,
        )?;
        let mut distinct = BTreeSet::new();
        let mut values = Vec::new();
        for (index, event) in events.into_iter().enumerate() {
            check_query_deadline(started, index)?;
            let public = public_event(&event, None);
            if !matches_nodes(&body.filters, Combination::And, &public)? {
                continue;
            }
            let Some(value) = field_value(&public, &body.key) else {
                continue;
            };
            if scalar_type(value) != Some(body.value_type) {
                continue;
            }
            let encoded = serde_json::to_string(value).map_err(|_| V4Error::Internal)?;
            if distinct.insert(encoded) {
                values.push(json!({"dataset": workers_logs_dataset(), "key": body.key,
                    "type": body.value_type.as_str(), "value": value}));
                if values.len() >= limit as usize {
                    break;
                }
            }
        }
        bounded_result(values)
    })();
    handlers::respond(context, result)
}

fn value_response(key: &str, value: &ObservabilityFieldValue) -> Value {
    json!({"dataset": workers_logs_dataset(), "key": key,
        "type": value.value_type, "value": value.value})
}

pub(super) async fn telemetry_query(
    State(state): State<HttpState>,
    Path(account): Path<String>,
    request: Request,
) -> Response {
    let context = match handlers::authorize(&request, V4Permission::Read) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let body: QueryBody = match super::super::json::json_body(request).await {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    let started = Instant::now();
    let result = query_result(&state, &account, &body, started, context.request_id());
    if let Ok(service) = observability(&state) {
        service.observe_query(
            matches!(body.view, QueryView::Invocations),
            result.is_ok(),
            started.elapsed(),
        );
    }
    handlers::respond(context, result)
}

fn query_result(
    state: &HttpState,
    public_account: &str,
    body: &QueryBody,
    started: Instant,
    request_id: RequestId,
) -> Result<Value, V4Error> {
    if body.query_id.is_empty() || body.query_id.len() > 256 {
        return Err(V4Error::InvalidField("/queryId"));
    }
    if body.chart.is_some()
        || body.chart_type.is_some()
        || body.compare.is_some()
        || body.granularity.is_some()
        || body.ignore_series.is_some()
        || body.offset_by.is_some()
        || body.offset_direction.is_some()
        || !matches!(body.view, QueryView::Events | QueryView::Invocations)
    {
        return Err(V4Error::Unsupported);
    }
    let parameters = body.parameters.as_ref().ok_or(V4Error::Unsupported)?;
    if parameters.calculations.is_some()
        || parameters.group_bys.is_some()
        || parameters.havings.is_some()
        || parameters.needle.is_some()
        || parameters.order_by.is_some()
    {
        return Err(V4Error::Unsupported);
    }
    validate_datasets(&parameters.datasets)?;
    validate_filters(&parameters.filters)?;
    let account_id = domain::resolve_account(state, public_account)?;
    let service = observability(state)?;
    validate_timeframe(service, body.timeframe.from, body.timeframe.to)?;
    let limit = body.limit.or(parameters.limit).unwrap_or(100);
    if limit == 0 || limit > service.config().query_max_events {
        return Err(V4Error::InvalidField("/limit"));
    }
    let cursor = body
        .offset
        .as_deref()
        .map(|value| {
            service
                .decode_cursor(
                    value,
                    account_id,
                    &body.query_id,
                    body.timeframe.from,
                    body.timeframe.to,
                )
                .map_err(|error| V4Error::from(&error))
        })
        .transpose()?;
    let events = candidates(
        service,
        account_id,
        body.timeframe.from,
        body.timeframe.to,
        cursor.as_ref(),
    )?;
    let rows_read = events.len();
    let mut selected = Vec::new();
    let mut selected_bytes = 0_usize;
    for (index, event) in events.into_iter().enumerate() {
        check_query_deadline(started, index)?;
        let cursor = service
            .encode_cursor(
                account_id,
                &body.query_id,
                body.timeframe.from,
                body.timeframe.to,
                &ObservabilityEventCursor {
                    timestamp_ms: event.timestamp_ms,
                    event_id: event.event_id.clone(),
                },
            )
            .map_err(|error| V4Error::from(&error))?;
        let public = public_event(&event, Some(cursor));
        if matches_nodes(&parameters.filters, parameters.filter_combination, &public)? {
            selected_bytes = selected_bytes.saturating_add(
                serde_json::to_vec(&public)
                    .map_err(|_| V4Error::Internal)?
                    .len(),
            );
            if selected_bytes > QUERY_RESULT_MAX_BYTES {
                return Err(V4Error::RateLimited);
            }
            selected.push(public);
            if selected.len() >= limit as usize {
                break;
            }
        }
    }
    let created = format_timestamp(now_ms().map_err(|error| V4Error::from(&error))?)?;
    let query = json!({
        "id": body.query_id,
        "adhoc": true,
        "created": created,
        "createdBy": "open-compute",
        "description": null,
        "name": body.query_id,
        "parameters": parameters,
        "updated": created,
        "updatedBy": "open-compute"
    });
    let run = json!({
        "id": RequestId::generate().to_string(),
        "accountId": public_account,
        "dry": body.dry,
        "granularity": 0,
        "query": query,
        "status": "COMPLETED",
        "timeframe": body.timeframe,
        "userId": "open-compute",
        "created": created,
        "updated": created
    });
    let bytes_read = selected
        .iter()
        .map(|value| serde_json::to_vec(value).map_or(0, |v| v.len()))
        .sum::<usize>();
    let statistics = json!({
        "bytes_read": bytes_read,
        "elapsed": started.elapsed().as_secs_f64(),
        "rows_read": rows_read
    });
    let mut result = json!({"run": run, "statistics": statistics});
    match body.view {
        QueryView::Events => {
            result["events"] = json!({"count": selected.len(), "events": selected});
        }
        QueryView::Invocations => {
            let mut grouped = BTreeMap::<String, Vec<Value>>::new();
            for event in selected {
                let request_id = event
                    .pointer("/$metadata/requestId")
                    .and_then(Value::as_str)
                    .ok_or(V4Error::Internal)?
                    .to_owned();
                grouped.entry(request_id).or_default().push(event);
            }
            result["invocations"] = serde_json::to_value(grouped).map_err(|_| V4Error::Internal)?;
        }
        QueryView::Traces | QueryView::Calculations | QueryView::Requests | QueryView::Agents => {
            return Err(V4Error::Unsupported);
        }
    }
    let result_count = match body.view {
        QueryView::Events => result
            .pointer("/events/count")
            .and_then(Value::as_u64)
            .and_then(|value| usize::try_from(value).ok())
            .ok_or(V4Error::Internal)?,
        QueryView::Invocations => result
            .get("invocations")
            .and_then(Value::as_object)
            .map_or(0, serde_json::Map::len),
        QueryView::Traces | QueryView::Calculations | QueryView::Requests | QueryView::Agents => {
            return Err(V4Error::Unsupported);
        }
    };
    let result = bounded_result(result)?;
    let mut filter_keys = BTreeSet::new();
    collect_filter_keys(&parameters.filters, &mut filter_keys);
    let filter_keys = filter_keys
        .into_iter()
        .map(|key| audit_filter_key(&key).to_owned())
        .collect::<BTreeSet<_>>();
    service
        .audit_query(
            account_id,
            &ObservabilityAudit::Query {
                view: match body.view {
                    QueryView::Events => "events",
                    QueryView::Invocations => "invocations",
                    QueryView::Traces
                    | QueryView::Calculations
                    | QueryView::Requests
                    | QueryView::Agents => return Err(V4Error::Unsupported),
                }
                .to_owned(),
                from_ms: body.timeframe.from,
                to_ms: body.timeframe.to,
                result_count,
                filter_keys: filter_keys.into_iter().collect(),
            },
            request_id,
        )
        .map_err(|error| V4Error::from(&error))?;
    Ok(result)
}

fn audit_filter_key(key: &str) -> &'static str {
    if key == "dataset" {
        "dataset"
    } else if key == "timestamp" {
        "timestamp"
    } else if key == "source" || key.starts_with("source.") {
        "source.*"
    } else if key == "$metadata" || key.starts_with("$metadata.") {
        "$metadata.*"
    } else if key == "$workers" || key.starts_with("$workers.") {
        "$workers.*"
    } else {
        "other"
    }
}

fn observability(state: &HttpState) -> Result<&Arc<ObservabilityService>, V4Error> {
    handlers::worker_api(state)?
        .observability()
        .map_err(|error| V4Error::from(&error))
}

fn candidates(
    service: &ObservabilityService,
    account_id: AccountId,
    from: i64,
    to: i64,
    cursor: Option<&ObservabilityEventCursor>,
) -> Result<Vec<StoredObservabilityEvent>, V4Error> {
    let events = service
        .store()
        .ok_or(V4Error::Unavailable)?
        .query_events(
            &account_id.to_string(),
            from,
            to,
            None,
            cursor,
            QUERY_CANDIDATES,
        )
        .map_err(|error| V4Error::from(&error))?;
    if events.len() == QUERY_CANDIDATES as usize {
        return Err(V4Error::RateLimited);
    }
    Ok(events)
}

fn check_query_deadline(started: Instant, index: usize) -> Result<(), V4Error> {
    if index.is_multiple_of(64) && started.elapsed() > QUERY_DEADLINE {
        Err(V4Error::RateLimited)
    } else {
        Ok(())
    }
}

fn bounded_result<T: Serialize>(value: T) -> Result<T, V4Error> {
    if serde_json::to_vec(&value)
        .map_err(|_| V4Error::Internal)?
        .len()
        > QUERY_RESULT_MAX_BYTES
    {
        Err(V4Error::RateLimited)
    } else {
        Ok(value)
    }
}

fn public_event(event: &StoredObservabilityEvent, cursor: Option<String>) -> Value {
    let mut metadata = event.metadata.as_object().cloned().unwrap_or_default();
    if let Some(cursor) = cursor {
        metadata.insert("id".to_owned(), Value::String(cursor));
    }
    let event_type = metadata
        .get("origin")
        .cloned()
        .unwrap_or(Value::String("unknown".to_owned()));
    json!({
        "$metadata": metadata,
        "$workers": {
            "scriptName": event.script_name,
            "scriptVersion": {"id": event.version_id},
            "eventType": event_type,
            "outcome": metadata.get("outcome").cloned().unwrap_or(Value::String("unknown".to_owned())),
            "cpuTimeMs": metadata.get("cpuTimeMs").cloned().unwrap_or(json!(0)),
            "wallTimeMs": metadata.get("wallTimeMs").cloned().unwrap_or(json!(0))
        },
        "dataset": workers_logs_dataset(),
        "source": event.source,
        "timestamp": event.timestamp_ms
    })
}

fn validate_datasets(values: &[String]) -> Result<(), V4Error> {
    if values.is_empty() || (values.len() == 1 && values[0] == workers_logs_dataset()) {
        Ok(())
    } else {
        Err(V4Error::Unsupported)
    }
}

fn validate_timeframe(service: &ObservabilityService, from: i64, to: i64) -> Result<(), V4Error> {
    let maximum = i64::try_from(service_config(service).query_max_timeframe_ms)
        .map_err(|_| V4Error::Internal)?;
    let retention =
        i64::try_from(service_config(service).retention_ms).map_err(|_| V4Error::Internal)?;
    let now = now_ms().map_err(|error| V4Error::from(&error))?;
    if from < now.saturating_sub(retention)
        || from >= to
        || to > now.saturating_add(60_000)
        || to.saturating_sub(from) > maximum
    {
        Err(V4Error::InvalidField("/timeframe"))
    } else {
        Ok(())
    }
}

fn service_config(
    service: &ObservabilityService,
) -> &open_compute_core::config::ObservabilityConfig {
    service.config()
}

fn validate_filters(filters: &[FilterNode]) -> Result<(), V4Error> {
    validate_filter_ast(filters).map_err(|error| V4Error::from(&error))
}

fn matches_nodes(
    filters: &[FilterNode],
    combination: Combination,
    event: &Value,
) -> Result<bool, V4Error> {
    filter_matches(filters, combination, event).map_err(|error| V4Error::from(&error))
}

fn format_timestamp(value: i64) -> Result<String, V4Error> {
    jiff::Timestamp::from_millisecond(value)
        .map(|value| value.to_string())
        .map_err(|_| V4Error::Internal)
}

fn now_ms() -> Result<i64, PlatformError> {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| {
            PlatformError::new(
                ErrorCode::PlatformUnavailable,
                "system clock is unavailable",
            )
        })?
        .as_millis();
    i64::try_from(millis).map_err(|_| {
        PlatformError::new(
            ErrorCode::PlatformUnavailable,
            "system clock is unavailable",
        )
    })
}

#[cfg(test)]
#[path = "observability_query_tests.rs"]
mod tests;
