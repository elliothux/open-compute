use super::accounts::AccountAuthority;
use super::*;
use crate::health::HealthCoordinator;
use crate::http::{HttpState, REQUEST_ID_HEADER};
use crate::metrics::MetricsRegistry;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
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
    router(state.clone(), Router::new()).with_state(state)
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
    let endpoint_count = capabilities["result"]["endpoints"]
        .as_object()
        .unwrap()
        .len();
    let authority_count = serde_json::from_slice::<serde_json::Value>(include_bytes!(
        "../../../../share/cloudflare-capabilities.json"
    ))
    .unwrap()["managementApi"]["routes"]
        .as_array()
        .unwrap()
        .len();
    assert_eq!(endpoint_count, authority_count);
    assert_eq!(
        capabilities["result"]["deviations"][0],
        "OC-MANAGEMENT-COMPATIBILITY-DATE-001"
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
