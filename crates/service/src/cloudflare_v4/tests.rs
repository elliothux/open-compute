use super::accounts::AccountAuthority;
use super::*;
use crate::health::HealthCoordinator;
use crate::http::{HttpState, REQUEST_ID_HEADER};
use crate::metrics::MetricsRegistry;
use axum::body::{Body, to_bytes};
use axum::http::{Method, Request, StatusCode, header};
use open_compute_core::config::MetricsConfig;
use open_compute_core::{AccountId, PlatformId, SecretString};
use std::sync::Arc;
use tower::ServiceExt as _;

fn state() -> (HttpState, AccountAuthority) {
    let authority = AccountAuthority::new(PlatformId::generate(), AccountId::generate(), 1_000);
    let metrics = Arc::new(
        MetricsRegistry::new(&MetricsConfig::default(), "test", "workerd")
            .expect("metrics registry"),
    );
    let state = HttpState::for_test(
        HealthCoordinator::new(),
        metrics,
        false,
        Some(SecretString::new("admin-token")),
    )
    .with_v4_tokens(
        SecretString::new("deployer-token"),
        SecretString::new("read-token"),
    )
    .with_cloudflare_v4_account(authority.clone());
    (state, authority)
}

fn app(state: HttpState) -> Router {
    router(state.clone(), storage_router()).with_state(state)
}

fn full_app(state: HttpState) -> Router {
    router(state.clone(), crate::workers_http::v4::router()).with_state(state)
}

async fn json(response: axum::response::Response) -> serde_json::Value {
    serde_json::from_slice(
        &to_bytes(response.into_body(), 64 * 1024)
            .await
            .expect("bounded body"),
    )
    .expect("JSON response")
}

#[tokio::test]
async fn authentication_is_fail_closed_and_all_responses_have_request_ids() {
    let (state, _) = state();
    let unauthenticated = app(state.clone())
        .oneshot(Request::builder().uri("/user").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(unauthenticated.status(), StatusCode::UNAUTHORIZED);
    assert!(unauthenticated.headers().contains_key(REQUEST_ID_HEADER));
    let body = json(unauthenticated).await;
    assert_eq!(body["success"], false);
    assert_eq!(body["result"], serde_json::Value::Null);

    for token in ["admin-token", "deployer-token", "read-token"] {
        let response = app(state.clone())
            .oneshot(
                Request::builder()
                    .uri("/user/tokens/verify")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(response.headers().contains_key(REQUEST_ID_HEADER));
        assert_eq!(json(response).await["result"]["status"], "active");
    }
}

#[tokio::test]
async fn account_collections_use_public_ids_and_sibling_result_info() {
    let (state, authority) = state();
    let response = app(state.clone())
        .oneshot(
            Request::builder()
                .uri("/accounts?page=1&per_page=20")
                .header(header::AUTHORIZATION, "Bearer read-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = json(response).await;
    assert_eq!(body["result"][0]["id"], authority.public_id());
    assert_eq!(body["result_info"]["total_count"], 1);
    assert!(body["result"][0]["id"].as_str().unwrap().len() == 32);

    let detail = app(state)
        .oneshot(
            Request::builder()
                .uri(format!("/accounts/{}", authority.public_id()))
                .header(header::AUTHORIZATION, "Bearer deployer-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(detail.status(), StatusCode::OK);
    assert_eq!(json(detail).await["result"]["type"], "standard");
}

#[tokio::test]
async fn vendor_capabilities_and_system_status_use_the_canonical_envelope() {
    let (state, _) = state();
    let capabilities = app(state.clone())
        .oneshot(
            Request::builder()
                .uri("/open-compute/capabilities")
                .header(header::AUTHORIZATION, "Bearer read-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(capabilities.status(), StatusCode::OK);
    let capabilities = json(capabilities).await;
    assert_eq!(capabilities["success"], true);
    assert_eq!(capabilities["result"]["wrangler_version"], "4.127.1");
    assert_eq!(
        capabilities["result"]["compatibility_date"]["minimum"],
        "2026-08-30"
    );
    assert_eq!(
        capabilities["result"]["compatibility_flags"],
        serde_json::json!(["nodejs_compat"])
    );
    let endpoint_count = capabilities["result"]["endpoints"]
        .as_object()
        .unwrap()
        .len();
    let authority_count = serde_json::from_slice::<serde_json::Value>(include_bytes!(
        "../../../../openapi/p6-capability.json"
    ))
    .unwrap()["managementApi"]["routes"]
        .as_array()
        .unwrap()
        .len();
    assert_eq!(endpoint_count, authority_count);
    let deviations = capabilities["result"]["deviations"].as_array().unwrap();
    assert_eq!(deviations[0], "OC-ACCOUNT-SUBDOMAIN-001");
    assert!(
        deviations
            .iter()
            .any(|value| value == "OC-OBSERVABILITY-001")
    );

    let status = app(state)
        .oneshot(
            Request::builder()
                .uri("/open-compute/system/status")
                .header(header::AUTHORIZATION, "Bearer read-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(status.status(), StatusCode::OK);
    let status = json(status).await;
    assert_eq!(status["success"], true);
    assert!(status["result"]["components"].as_array().unwrap().len() > 5);
}

#[tokio::test]
async fn bodyless_vendor_posts_reject_content() {
    let (state, _) = state();
    let response = app(state)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/open-compute/scheduler/pause")
                .header(header::AUTHORIZATION, "Bearer admin-token")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(json(response).await["errors"][0]["code"], 9_100_003);
}

#[tokio::test]
async fn backup_routes_authenticate_before_parsing_and_reject_get_bodies() {
    let (state, authority) = state();
    let path = format!(
        "/accounts/{}/open-compute/kv/namespaces/00000000000000000000000000000000/backups",
        authority.public_id()
    );
    let unauthenticated = app(state.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(&path)
                .header("idempotency-key", "has space")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(unauthenticated.status(), StatusCode::UNAUTHORIZED);

    let malformed = app(state.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(&path)
                .header(header::AUTHORIZATION, "Bearer admin-token")
                .header("idempotency-key", "has space")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(malformed.status(), StatusCode::BAD_REQUEST);

    let duplicate_idempotency = app(state.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(&path)
                .header(header::AUTHORIZATION, "Bearer admin-token")
                .header("idempotency-key", "first")
                .header("idempotency-key", "second")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(duplicate_idempotency.status(), StatusCode::BAD_REQUEST);

    let restore_path = format!(
        "/accounts/{}/open-compute/kv/backups/backup/restore",
        authority.public_id()
    );
    let duplicate_content_type = app(state.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(restore_path)
                .header(header::AUTHORIZATION, "Bearer admin-token")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::CONTENT_TYPE, "application/json; charset=utf-8")
                .body(Body::from(r#"{"name":"restored"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(duplicate_content_type.status(), StatusCode::BAD_REQUEST);

    let get_with_body = app(state)
        .oneshot(
            Request::builder()
                .uri(path)
                .header(header::AUTHORIZATION, "Bearer read-token")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(get_with_body.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn duplicate_role_tokens_never_resolve_to_a_role() {
    let (state, authority) = state();
    let state = HttpState::for_test(
        HealthCoordinator::new(),
        state.metrics().clone(),
        false,
        Some(SecretString::new("same-token")),
    )
    .with_v4_tokens(
        SecretString::new("same-token"),
        SecretString::new("read-token"),
    )
    .with_cloudflare_v4_account(authority);
    let response = app(state)
        .oneshot(
            Request::builder()
                .uri("/user")
                .header(header::AUTHORIZATION, "Bearer same-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn permission_and_query_errors_never_use_authentication_code() {
    let (state, _) = state();
    let denied = app(state.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/open-compute/scheduler/pause")
                .header(header::AUTHORIZATION, "Bearer deployer-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);
    let denied_body = json(denied).await;
    assert_eq!(denied_body["errors"][0]["code"], 9_100_002);
    assert_ne!(denied_body["errors"][0]["code"], 10_000);

    let invalid = app(state)
        .oneshot(
            Request::builder()
                .uri("/accounts?per_page=1")
                .header(header::AUTHORIZATION, "Bearer read-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);
    let invalid_body = json(invalid).await;
    assert_eq!(invalid_body["errors"][0]["code"], 9_100_003);
    assert_ne!(invalid_body["errors"][0]["code"], 10_000);
}

#[tokio::test]
async fn storage_boundaries_return_cloudflare_errors_before_domain_dispatch() {
    let (state, authority) = state();
    let invalid_query = app(state.clone())
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/accounts/{}/storage/kv/namespaces?page=1&page=2",
                    authority.public_id()
                ))
                .header(header::AUTHORIZATION, "Bearer read-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(invalid_query.status(), StatusCode::BAD_REQUEST);
    assert!(invalid_query.headers().contains_key(REQUEST_ID_HEADER));
    assert_eq!(json(invalid_query).await["errors"][0]["code"], 9_100_003);

    let forbidden_query = app(state.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/accounts/{}/storage/kv/namespaces?unknown=true",
                    authority.public_id()
                ))
                .header(header::AUTHORIZATION, "Bearer deployer-token")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"title":"namespace"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(forbidden_query.status(), StatusCode::BAD_REQUEST);
    assert!(forbidden_query.headers().contains_key(REQUEST_ID_HEADER));
    assert_eq!(json(forbidden_query).await["errors"][0]["code"], 9_100_003);

    let denied = app(state.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/accounts/{}/storage/kv/namespaces",
                    authority.public_id()
                ))
                .header(header::AUTHORIZATION, "Bearer read-token")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"title":"namespace"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);
    assert_eq!(json(denied).await["errors"][0]["code"], 9_100_002);

    let unsupported = app(state)
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/accounts/{}/r2/buckets/valid-bucket/objects",
                    authority.public_id()
                ))
                .header(header::AUTHORIZATION, "Bearer read-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(unsupported.status(), StatusCode::NOT_IMPLEMENTED);
    assert_eq!(json(unsupported).await["errors"][0]["code"], 9_100_007);
}

#[tokio::test]
async fn d1_transfer_scope_media_and_query_contracts_fail_closed_before_authority() {
    let (state, authority) = state();
    let base = format!(
        "/accounts/{}/d1/database/00000000000000000000000000000000",
        authority.public_id()
    );
    let read_only = app(state.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("{base}/export"))
                .header(header::AUTHORIZATION, "Bearer read-token")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"output_format":"polling"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(read_only.status(), StatusCode::FORBIDDEN);
    assert_eq!(json(read_only).await["errors"][0]["code"], 9_100_002);

    let duplicate_media = app(state.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("{base}/import"))
                .header(header::AUTHORIZATION, "Bearer deployer-token")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::CONTENT_TYPE, "application/json; charset=utf-8")
                .body(Body::from(r#"{"action":"init","etag":"00"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(duplicate_media.status(), StatusCode::BAD_REQUEST);

    let duplicate_timestamp = app(state.clone())
        .oneshot(
            Request::builder()
                .uri(format!(
                    "{base}/time_travel/bookmark?timestamp=2026-01-01T00%3A00%3A00Z&timestamp=2026-01-02T00%3A00%3A00Z"
                ))
                .header(header::AUTHORIZATION, "Bearer read-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(duplicate_timestamp.status(), StatusCode::BAD_REQUEST);

    let ambiguous_restore = app(state)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "{base}/time_travel/restore?bookmark=opaque&timestamp=2026-01-01T00%3A00%3A00Z"
                ))
                .header(header::AUTHORIZATION, "Bearer deployer-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(ambiguous_restore.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn authenticated_surface_fails_closed_without_product_authorities() {
    struct Case {
        method: Method,
        path: String,
        content_type: Option<&'static str>,
        body: &'static str,
    }

    let (state, authority) = state();
    let account = authority.public_id();
    let resource = "00000000000000000000000000000000";
    let cases = [
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
            method: Method::DELETE,
            path: format!("/accounts/{account}/queues/{resource}"),
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
    ];

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

#[test]
fn scope_matrix_is_minimal_and_explicit() {
    let context = |role| V4RequestContext {
        role,
        request_id: open_compute_core::RequestId::generate(),
    };
    for permission in [
        V4Permission::Read,
        V4Permission::ProductWrite,
        V4Permission::Maintenance,
    ] {
        assert!(context(V4Role::Admin).require(permission).is_ok());
    }
    assert!(
        context(V4Role::Deployer)
            .require(V4Permission::Read)
            .is_ok()
    );
    assert!(
        context(V4Role::Deployer)
            .require(V4Permission::ProductWrite)
            .is_ok()
    );
    assert!(
        context(V4Role::Deployer)
            .require(V4Permission::Maintenance)
            .is_err()
    );
    assert!(
        context(V4Role::ReadOnly)
            .require(V4Permission::Read)
            .is_ok()
    );
    assert!(
        context(V4Role::ReadOnly)
            .require(V4Permission::ProductWrite)
            .is_err()
    );
}
