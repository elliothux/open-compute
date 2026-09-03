use super::*;
use axum::http::HeaderValue;

#[test]
fn wrangler_queue_list_query_is_strict_and_bounded() {
    let query = ListQuery::parse(Some("page=2&name=alpha")).unwrap();
    assert_eq!(query.page, 2);
    assert_eq!(query.per_page, 20);
    assert_eq!(query.name.as_deref(), Some("alpha"));
    assert!(ListQuery::parse(Some("cursor=legacy")).is_err());
    assert!(ListQuery::parse(Some("per_page=10")).is_err());
    assert!(ListQuery::parse(Some("name=alpha&name=beta")).is_err());
}

#[test]
fn queue_json_contract_rejects_unknown_fields() {
    let create: CreateQueueBody = serde_json::from_str(
        r#"{"queue_name":"jobs","settings":{"delivery_delay":5,"delivery_paused":true,"message_retention_period":3600}}"#,
    )
    .unwrap();
    assert_eq!(create.queue_name, "jobs");
    assert_eq!(create.settings.unwrap().delivery_delay, Some(5));
    assert!(
        serde_json::from_str::<CreateQueueBody>(r#"{"queue_name":"jobs","legacy":true}"#).is_err()
    );
}

#[test]
fn queue_json_headers_accept_fetch_string_bodies_without_relaxing_duplicates() {
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/plain;charset=UTF-8"),
    );
    validate_json_headers(&headers).unwrap();
    headers.append(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    assert!(validate_json_headers(&headers).is_err());
}

#[test]
fn worker_consumer_json_accepts_pinned_wrangler_shape_only() {
    let body: consumers::ConsumerBody = serde_json::from_str(
        r#"{"type":"worker","script_name":"processor","environment_name":"","settings":{"batch_size":10,"max_wait_time_ms":5000}}"#,
    )
    .unwrap();
    assert!(matches!(body.kind, consumers::ConsumerKind::Worker));
    assert_eq!(body.script_name, "processor");
    assert!(
        serde_json::from_str::<consumers::ConsumerBody>(
            r#"{"type":"worker","script_name":"processor","visibility_timeout_ms":1000}"#,
        )
        .is_err()
    );
}

#[test]
fn worker_consumer_settings_enforce_whole_seconds_and_local_concurrency() {
    let valid = consumers::settings(
        Some(consumers::ConsumerSettingsBody {
            batch_size: Some(100),
            max_concurrency: Some(8),
            max_retries: Some(100),
            max_wait_time_ms: Some(60_000),
            retry_delay: Some(86_400),
        }),
        8,
    )
    .unwrap();
    assert_eq!(valid.max_batch_timeout_seconds, 60);
    assert!(
        consumers::settings(
            Some(consumers::ConsumerSettingsBody {
                max_wait_time_ms: Some(1),
                ..Default::default()
            }),
            8,
        )
        .is_err()
    );
    assert!(
        consumers::settings(
            Some(consumers::ConsumerSettingsBody {
                max_concurrency: Some(9),
                ..Default::default()
            }),
            8,
        )
        .is_err()
    );
}
