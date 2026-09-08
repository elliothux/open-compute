use super::*;

#[derive(Clone)]
struct Case {
    method: Method,
    path: String,
    content_type: Option<&'static str>,
    body: &'static str,
}

#[tokio::test]
async fn authenticated_surface_fails_closed_without_product_authorities() {
    let (state, authority) = state();
    let account = authority.public_id();
    let resource = "00000000000000000000000000000000";
    let cases = [
        storage_cases(account, resource),
        queue_and_workflow_cases(account, resource),
        worker_and_platform_cases(account, resource),
    ]
    .concat();

    for case in cases {
        let mut builder = Request::builder()
            .method(case.method.clone())
            .uri(&case.path)
            .header(header::AUTHORIZATION, "Bearer admin-token")
            .header(header::HOST, "127.0.0.1:8787");
        if let Some(content_type) = case.content_type {
            builder = builder.header(header::CONTENT_TYPE, content_type);
        }
        let response = full_app(state.clone())
            .oneshot(builder.body(Body::from(case.body)).unwrap())
            .await
            .unwrap();
        assert!(
            matches!(
                response.status(),
                StatusCode::BAD_REQUEST
                    | StatusCode::FORBIDDEN
                    | StatusCode::NOT_FOUND
                    | StatusCode::NOT_IMPLEMENTED
                    | StatusCode::CONFLICT
                    | StatusCode::SERVICE_UNAVAILABLE
            ),
            "unexpected status for {}: {}",
            case.path,
            response.status()
        );
        assert!(response.headers().contains_key(REQUEST_ID_HEADER));
        let body = json(response).await;
        assert_eq!(body["success"], false, "unexpected body for {}", case.path);
        assert!(body["errors"][0]["code"].is_number());
    }
}

fn storage_cases(account: &str, resource: &str) -> Vec<Case> {
    vec![
        Case {
            method: Method::POST,
            path: format!("/accounts/{account}/storage/kv/namespaces"),
            content_type: Some("application/json"),
            body: r#"{"title":"coverage-kv"}"#,
        },
        Case {
            method: Method::GET,
            path: format!("/accounts/{account}/storage/kv/namespaces"),
            content_type: None,
            body: "",
        },
        Case {
            method: Method::GET,
            path: format!("/accounts/{account}/storage/kv/namespaces/{resource}"),
            content_type: None,
            body: "",
        },
        Case {
            method: Method::PUT,
            path: format!("/accounts/{account}/storage/kv/namespaces/{resource}"),
            content_type: Some("application/json"),
            body: r#"{"title":"coverage-renamed"}"#,
        },
        Case {
            method: Method::DELETE,
            path: format!("/accounts/{account}/storage/kv/namespaces/{resource}"),
            content_type: None,
            body: "",
        },
        Case {
            method: Method::GET,
            path: format!("/accounts/{account}/storage/kv/namespaces/{resource}/keys"),
            content_type: None,
            body: "",
        },
        Case {
            method: Method::GET,
            path: format!("/accounts/{account}/storage/kv/namespaces/{resource}/values/key"),
            content_type: None,
            body: "",
        },
        Case {
            method: Method::PUT,
            path: format!("/accounts/{account}/storage/kv/namespaces/{resource}/values/key"),
            content_type: Some("application/octet-stream"),
            body: "value",
        },
        Case {
            method: Method::DELETE,
            path: format!("/accounts/{account}/storage/kv/namespaces/{resource}/values/key"),
            content_type: None,
            body: "",
        },
        Case {
            method: Method::GET,
            path: format!("/accounts/{account}/storage/kv/namespaces/{resource}/metadata/key"),
            content_type: None,
            body: "",
        },
        Case {
            method: Method::PUT,
            path: format!("/accounts/{account}/storage/kv/namespaces/{resource}/bulk"),
            content_type: Some("application/json"),
            body: "[]",
        },
        Case {
            method: Method::POST,
            path: format!("/accounts/{account}/storage/kv/namespaces/{resource}/bulk/get"),
            content_type: Some("application/json"),
            body: "[]",
        },
        Case {
            method: Method::POST,
            path: format!("/accounts/{account}/storage/kv/namespaces/{resource}/bulk/delete"),
            content_type: Some("application/json"),
            body: "[]",
        },
        Case {
            method: Method::POST,
            path: format!("/accounts/{account}/d1/database"),
            content_type: Some("application/json"),
            body: r#"{"name":"coverage-db"}"#,
        },
        Case {
            method: Method::GET,
            path: format!("/accounts/{account}/d1/database"),
            content_type: None,
            body: "",
        },
        Case {
            method: Method::GET,
            path: format!("/accounts/{account}/d1/database/{resource}"),
            content_type: None,
            body: "",
        },
        Case {
            method: Method::PUT,
            path: format!("/accounts/{account}/d1/database/{resource}"),
            content_type: Some("application/json"),
            body: r#"{"name":"coverage-db"}"#,
        },
        Case {
            method: Method::DELETE,
            path: format!("/accounts/{account}/d1/database/{resource}"),
            content_type: None,
            body: "",
        },
        Case {
            method: Method::POST,
            path: format!("/accounts/{account}/d1/database/{resource}/query"),
            content_type: Some("application/json"),
            body: r#"{"sql":"SELECT 1"}"#,
        },
        Case {
            method: Method::POST,
            path: format!("/accounts/{account}/d1/database/{resource}/raw"),
            content_type: Some("application/json"),
            body: r#"{"sql":"SELECT 1"}"#,
        },
        Case {
            method: Method::POST,
            path: format!("/accounts/{account}/d1/database/{resource}/export"),
            content_type: Some("application/json"),
            body: r#"{"output_format":"polling"}"#,
        },
        Case {
            method: Method::POST,
            path: format!("/accounts/{account}/d1/database/{resource}/import"),
            content_type: Some("application/json"),
            body: r#"{"action":"init","etag":"00"}"#,
        },
        Case {
            method: Method::GET,
            path: format!(
                "/accounts/{account}/d1/database/{resource}/time_travel/bookmark?timestamp=2026-01-01T00%3A00%3A00Z"
            ),
            content_type: None,
            body: "",
        },
        Case {
            method: Method::POST,
            path: format!(
                "/accounts/{account}/d1/database/{resource}/time_travel/restore?bookmark=opaque"
            ),
            content_type: None,
            body: "",
        },
    ]
}

fn queue_and_workflow_cases(account: &str, resource: &str) -> Vec<Case> {
    vec![
        Case {
            method: Method::GET,
            path: format!("/accounts/{account}/queues"),
            content_type: None,
            body: "",
        },
        Case {
            method: Method::POST,
            path: format!("/accounts/{account}/queues"),
            content_type: Some("application/json"),
            body: r#"{"queue_name":"coverage-queue"}"#,
        },
        Case {
            method: Method::GET,
            path: format!("/accounts/{account}/queues/{resource}"),
            content_type: None,
            body: "",
        },
        Case {
            method: Method::PUT,
            path: format!("/accounts/{account}/queues/{resource}"),
            content_type: Some("application/json"),
            body: "{}",
        },
        Case {
            method: Method::PATCH,
            path: format!("/accounts/{account}/queues/{resource}"),
            content_type: Some("application/json"),
            body: "{}",
        },
        Case {
            method: Method::DELETE,
            path: format!("/accounts/{account}/queues/{resource}"),
            content_type: None,
            body: "",
        },
        Case {
            method: Method::GET,
            path: format!("/accounts/{account}/queues/{resource}/metrics"),
            content_type: None,
            body: "",
        },
        Case {
            method: Method::GET,
            path: format!("/accounts/{account}/queues/{resource}/consumers"),
            content_type: None,
            body: "",
        },
        Case {
            method: Method::POST,
            path: format!("/accounts/{account}/queues/{resource}/consumers"),
            content_type: Some("application/json"),
            body: r#"{"type":"worker","script_name":"worker"}"#,
        },
        Case {
            method: Method::GET,
            path: format!("/accounts/{account}/queues/{resource}/consumers/{resource}"),
            content_type: None,
            body: "",
        },
        Case {
            method: Method::PUT,
            path: format!("/accounts/{account}/queues/{resource}/consumers/{resource}"),
            content_type: Some("application/json"),
            body: r#"{"type":"worker","script_name":"worker"}"#,
        },
        Case {
            method: Method::DELETE,
            path: format!("/accounts/{account}/queues/{resource}/consumers/{resource}"),
            content_type: None,
            body: "",
        },
        Case {
            method: Method::POST,
            path: format!("/accounts/{account}/r2/buckets"),
            content_type: Some("application/json"),
            body: r#"{"name":"coverage-bucket"}"#,
        },
        Case {
            method: Method::GET,
            path: format!("/accounts/{account}/r2/buckets"),
            content_type: None,
            body: "",
        },
        Case {
            method: Method::PUT,
            path: format!("/accounts/{account}/r2/buckets/coverage-bucket"),
            content_type: None,
            body: "",
        },
        Case {
            method: Method::GET,
            path: format!("/accounts/{account}/r2/buckets/coverage-bucket"),
            content_type: None,
            body: "",
        },
        Case {
            method: Method::DELETE,
            path: format!("/accounts/{account}/r2/buckets/coverage-bucket"),
            content_type: None,
            body: "",
        },
        Case {
            method: Method::GET,
            path: format!("/accounts/{account}/r2/buckets/coverage-bucket/objects"),
            content_type: None,
            body: "",
        },
        Case {
            method: Method::GET,
            path: format!("/accounts/{account}/r2/buckets/coverage-bucket/objects/key"),
            content_type: None,
            body: "",
        },
        Case {
            method: Method::PUT,
            path: format!("/accounts/{account}/r2/buckets/coverage-bucket/objects/key"),
            content_type: Some("application/octet-stream"),
            body: "value",
        },
        Case {
            method: Method::DELETE,
            path: format!("/accounts/{account}/r2/buckets/coverage-bucket/objects/key"),
            content_type: None,
            body: "",
        },
        Case {
            method: Method::GET,
            path: format!("/accounts/{account}/workflows"),
            content_type: None,
            body: "",
        },
        Case {
            method: Method::GET,
            path: format!("/accounts/{account}/workflows/workflow"),
            content_type: None,
            body: "",
        },
        Case {
            method: Method::PUT,
            path: format!("/accounts/{account}/workflows/workflow"),
            content_type: Some("application/json"),
            body: "{}",
        },
        Case {
            method: Method::DELETE,
            path: format!("/accounts/{account}/workflows/workflow"),
            content_type: None,
            body: "",
        },
        Case {
            method: Method::GET,
            path: format!("/accounts/{account}/workflows/workflow/versions"),
            content_type: None,
            body: "",
        },
        Case {
            method: Method::GET,
            path: format!("/accounts/{account}/workflows/workflow/versions/{resource}"),
            content_type: None,
            body: "",
        },
        Case {
            method: Method::GET,
            path: format!("/accounts/{account}/workflows/workflow/instances"),
            content_type: None,
            body: "",
        },
        Case {
            method: Method::POST,
            path: format!("/accounts/{account}/workflows/workflow/instances"),
            content_type: Some("application/json"),
            body: "{}",
        },
        Case {
            method: Method::POST,
            path: format!("/accounts/{account}/workflows/workflow/instances/batch"),
            content_type: Some("application/json"),
            body: "[]",
        },
        Case {
            method: Method::GET,
            path: format!("/accounts/{account}/workflows/workflow/instances/{resource}"),
            content_type: None,
            body: "",
        },
        Case {
            method: Method::PATCH,
            path: format!("/accounts/{account}/workflows/workflow/instances/{resource}/status"),
            content_type: Some("application/json"),
            body: "{}",
        },
        Case {
            method: Method::POST,
            path: format!(
                "/accounts/{account}/workflows/workflow/instances/{resource}/events/event"
            ),
            content_type: Some("application/json"),
            body: "{}",
        },
    ]
}

fn worker_and_platform_cases(account: &str, resource: &str) -> Vec<Case> {
    vec![
        Case {
            method: Method::GET,
            path: format!("/accounts/{account}/workers/scripts"),
            content_type: None,
            body: "",
        },
        Case {
            method: Method::GET,
            path: format!("/accounts/{account}/workers/scripts/worker"),
            content_type: None,
            body: "",
        },
        Case {
            method: Method::DELETE,
            path: format!("/accounts/{account}/workers/scripts/worker"),
            content_type: None,
            body: "",
        },
        Case {
            method: Method::GET,
            path: format!("/accounts/{account}/workers/scripts/worker/versions"),
            content_type: None,
            body: "",
        },
        Case {
            method: Method::GET,
            path: format!("/accounts/{account}/workers/scripts/worker/versions/{resource}"),
            content_type: None,
            body: "",
        },
        Case {
            method: Method::GET,
            path: format!("/accounts/{account}/workers/scripts/worker/deployments"),
            content_type: None,
            body: "",
        },
        Case {
            method: Method::GET,
            path: format!("/accounts/{account}/workers/scripts/worker/deployments/{resource}"),
            content_type: None,
            body: "",
        },
        Case {
            method: Method::GET,
            path: format!("/accounts/{account}/workers/scripts/worker/script-settings"),
            content_type: None,
            body: "",
        },
        Case {
            method: Method::PATCH,
            path: format!("/accounts/{account}/workers/scripts/worker/script-settings"),
            content_type: Some("application/json"),
            body: "{}",
        },
        Case {
            method: Method::GET,
            path: format!("/accounts/{account}/workers/scripts/worker/settings"),
            content_type: None,
            body: "",
        },
        Case {
            method: Method::GET,
            path: format!("/accounts/{account}/workers/scripts/worker/secrets"),
            content_type: None,
            body: "",
        },
        Case {
            method: Method::GET,
            path: format!("/accounts/{account}/workers/scripts/worker/schedules"),
            content_type: None,
            body: "",
        },
        Case {
            method: Method::GET,
            path: format!("/accounts/{account}/workers/scripts/worker/subdomain"),
            content_type: None,
            body: "",
        },
        Case {
            method: Method::GET,
            path: format!("/accounts/{account}/workers/scripts/worker/tails"),
            content_type: None,
            body: "",
        },
        Case {
            method: Method::POST,
            path: format!("/accounts/{account}/workers/observability/telemetry/keys"),
            content_type: Some("application/json"),
            body: "{}",
        },
        Case {
            method: Method::POST,
            path: format!("/accounts/{account}/workers/observability/telemetry/values"),
            content_type: Some("application/json"),
            body: "{}",
        },
        Case {
            method: Method::POST,
            path: format!("/accounts/{account}/workers/observability/telemetry/query"),
            content_type: Some("application/json"),
            body: "{}",
        },
        Case {
            method: Method::POST,
            path: format!("/accounts/{account}/workers/observability/telemetry/live-tail"),
            content_type: Some("application/json"),
            body: "{}",
        },
        Case {
            method: Method::POST,
            path: format!(
                "/accounts/{account}/workers/observability/telemetry/live-tail/heartbeat"
            ),
            content_type: Some("application/json"),
            body: "{}",
        },
        Case {
            method: Method::GET,
            path: "/open-compute/scheduler".to_owned(),
            content_type: None,
            body: "",
        },
        Case {
            method: Method::POST,
            path: "/open-compute/scheduler/resume".to_owned(),
            content_type: None,
            body: "",
        },
        Case {
            method: Method::POST,
            path: "/open-compute/scheduler/repair".to_owned(),
            content_type: None,
            body: "",
        },
        Case {
            method: Method::GET,
            path: "/open-compute/cache".to_owned(),
            content_type: None,
            body: "",
        },
        Case {
            method: Method::POST,
            path: "/open-compute/cache/garbage-collection".to_owned(),
            content_type: None,
            body: "",
        },
        Case {
            method: Method::GET,
            path: "/open-compute/images/capacity".to_owned(),
            content_type: None,
            body: "",
        },
        Case {
            method: Method::GET,
            path: format!("/accounts/{account}/open-compute/workers/worker/endpoints"),
            content_type: None,
            body: "",
        },
        Case {
            method: Method::GET,
            path: format!("/accounts/{account}/open-compute/durable-objects"),
            content_type: None,
            body: "",
        },
        Case {
            method: Method::GET,
            path: format!("/accounts/{account}/open-compute/durable-objects/{resource}/objects"),
            content_type: None,
            body: "",
        },
    ]
}
