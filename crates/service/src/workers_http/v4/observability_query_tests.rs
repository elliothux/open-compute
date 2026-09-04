use super::*;
use axum::body::{Body, to_bytes};
use axum::http::{Method, Request, StatusCode, header};
use open_compute_core::{PlatformId, SecretString};
use open_compute_storage::{NewObservabilityEvent, NewObservabilityInvocation, ObservabilityField};
use tower::ServiceExt as _;

async fn json(response: Response) -> Value {
    serde_json::from_slice(
        &to_bytes(response.into_body(), 2 * 1024 * 1024)
            .await
            .unwrap(),
    )
    .unwrap()
}

fn request(uri: &str, body: &Value) -> Request<Body> {
    Request::builder()
        .method(Method::POST)
        .uri(uri)
        .header(header::AUTHORIZATION, "Bearer read-token")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(body).unwrap()))
        .unwrap()
}

#[test]
fn query_projection_helpers_cover_public_defaults_and_audit_buckets() {
    assert!(matches!(default_view(), QueryView::Events));
    for (key, expected) in [
        ("dataset", "dataset"),
        ("timestamp", "timestamp"),
        ("source.message", "source.*"),
        ("$metadata.requestId", "$metadata.*"),
        ("$workers.outcome", "$workers.*"),
        ("custom", "other"),
    ] {
        assert_eq!(audit_filter_key(key), expected);
    }
    assert!(validate_datasets(&[]).is_ok());
    assert!(validate_datasets(&[workers_logs_dataset().to_owned()]).is_ok());
    assert!(validate_datasets(&["other".to_owned()]).is_err());
    assert!(format_timestamp(0).unwrap().starts_with("1970-01-01"));
    assert!(format_timestamp(i64::MAX).is_err());
    assert!(check_query_deadline(Instant::now(), 1).is_ok());
    assert_eq!(
        value_response(
            "source.message",
            &ObservabilityFieldValue {
                value_type: "string".to_owned(),
                value: json!("hello"),
            }
        )["value"],
        "hello"
    );

    let event = StoredObservabilityEvent {
        event_id: "event".to_owned(),
        invocation_id: "invocation".to_owned(),
        script_name: "worker".to_owned(),
        version_id: "version".to_owned(),
        timestamp_ms: 1,
        sequence: 0,
        metadata_type: "cf-worker-event".to_owned(),
        level: None,
        source: json!({"message":"hello"}),
        metadata: json!({"requestId":"invocation"}),
    };
    let projected = public_event(&event, Some("cursor".to_owned()));
    assert_eq!(projected["$metadata"]["id"], "cursor");
    assert_eq!(projected["$workers"]["eventType"], "unknown");
    assert_eq!(projected["$workers"]["cpuTimeMs"], 0);
}

#[tokio::test]
async fn telemetry_keys_values_events_and_invocations_query_persisted_events() {
    let (_temp, _mock, state, account, _storage) =
        crate::tests::initialized_worker_http_fixture().await;
    let authority =
        crate::cloudflare_v4::accounts::AccountAuthority::new(PlatformId::generate(), account, 1);
    let public_account = authority.public_id().to_owned();
    let configured = state
        .with_v4_tokens(
            SecretString::new("deployer-token"),
            SecretString::new("read-token"),
        )
        .with_cloudflare_v4_account(authority);
    let service = configured
        .worker_api()
        .unwrap()
        .observability()
        .unwrap()
        .clone();
    let now = now_ms().unwrap();
    service
        .store()
        .unwrap()
        .insert(&NewObservabilityInvocation {
            invocation_id: "invocation-1".to_owned(),
            account_id: account.to_string(),
            script_name: "query-worker".to_owned(),
            version_id: "version-1".to_owned(),
            deployment_id: None,
            event_timestamp_ms: now,
            received_at_ms: now,
            event_type: "fetch".to_owned(),
            outcome: "ok".to_owned(),
            cpu_time_ms: 1.0,
            wall_time_ms: 2.0,
            truncated: false,
            event: json!({"request":{"method":"GET"}}),
            events: vec![NewObservabilityEvent {
                event_id: "event-1".to_owned(),
                sequence: 0,
                timestamp_ms: now,
                metadata_type: "cf-worker-log".to_owned(),
                level: Some("info".to_owned()),
                source: json!({"message":"invoice paid", "count":7}),
                metadata: json!({
                    "requestId":"invocation-1",
                    "origin":"fetch",
                    "outcome":"ok",
                    "cpuTimeMs":1,
                    "wallTimeMs":2
                }),
                fields: vec![
                    ObservabilityField {
                        key: "source.message".to_owned(),
                        value: json!("invoice paid"),
                    },
                    ObservabilityField {
                        key: "source.count".to_owned(),
                        value: json!(7),
                    },
                ],
            }],
        })
        .unwrap();
    let app = crate::http::admin_router(configured);
    let prefix = format!("/client/v4/accounts/{public_account}/workers/observability/telemetry");
    let from = now - 1_000;
    let to = now + 1_000;

    let keys = app
        .clone()
        .oneshot(request(
            &format!("{prefix}/keys"),
            &json!({"datasets":[workers_logs_dataset()],"from":from,"to":to,"limit":100}),
        ))
        .await
        .unwrap();
    assert_eq!(keys.status(), StatusCode::OK);
    assert!(json(keys).await["result"].as_array().unwrap().len() >= 2);

    let values = app
        .clone()
        .oneshot(request(
            &format!("{prefix}/values"),
            &json!({
                "datasets":[workers_logs_dataset()],
                "key":"source.message",
                "timeframe":{"from":from,"to":to},
                "type":"string",
                "limit":100
            }),
        ))
        .await
        .unwrap();
    assert_eq!(values.status(), StatusCode::OK);
    assert_eq!(json(values).await["result"][0]["value"], "invoice paid");

    let filter = json!({
        "key":"source.message",
        "operation":"includes",
        "type":"string",
        "value":"paid"
    });
    let filtered_keys = app
        .clone()
        .oneshot(request(
            &format!("{prefix}/keys"),
            &json!({
                "datasets":[workers_logs_dataset()],
                "filters":[filter.clone()],
                "from":from,
                "to":to,
                "limit":100
            }),
        ))
        .await
        .unwrap();
    assert_eq!(filtered_keys.status(), StatusCode::OK);
    assert!(
        json(filtered_keys).await["result"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["key"] == "source.message")
    );

    let filtered_values = app
        .clone()
        .oneshot(request(
            &format!("{prefix}/values"),
            &json!({
                "datasets":[workers_logs_dataset()],
                "key":"source.message",
                "timeframe":{"from":from,"to":to},
                "type":"string",
                "filters":[filter.clone()],
                "limit":100
            }),
        ))
        .await
        .unwrap();
    assert_eq!(filtered_values.status(), StatusCode::OK);
    assert_eq!(
        json(filtered_values).await["result"][0]["value"],
        "invoice paid"
    );

    for view in ["events", "invocations"] {
        let queried = app
            .clone()
            .oneshot(request(
                &format!("{prefix}/query"),
                &json!({
                    "queryId":format!("query-{view}"),
                    "timeframe":{"from":from,"to":to},
                    "parameters":{
                        "datasets":[workers_logs_dataset()],
                        "filterCombination":"AND",
                        "filters":[filter.clone()],
                        "limit":10
                    },
                    "view":view,
                    "dry":false
                }),
            ))
            .await
            .unwrap();
        assert_eq!(queried.status(), StatusCode::OK, "{view}");
        let body = json(queried).await;
        assert!(body["result"].get(view).is_some());
    }

    for body in [
        json!({"queryId":"","timeframe":{"from":from,"to":to},"parameters":{}}),
        json!({"queryId":"q","timeframe":{"from":to,"to":from},"parameters":{}}),
        json!({"queryId":"q","timeframe":{"from":from,"to":to},"view":"traces","parameters":{}}),
        json!({"queryId":"q","timeframe":{"from":from,"to":to},"chart":true,"parameters":{}}),
        json!({"queryId":"q","timeframe":{"from":from,"to":to},"parameters":{"calculations":[]}}),
        json!({"queryId":"q","timeframe":{"from":from,"to":to},"limit":0,"parameters":{}}),
    ] {
        let response = app
            .clone()
            .oneshot(request(&format!("{prefix}/query"), &body))
            .await
            .unwrap();
        assert!(!response.status().is_success());
    }
}
