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

mod authentication_is_fail_closed_and_all_responses_have_request_ids;

mod account_collections_use_public_ids_and_sibling_result_info;

mod vendor_capabilities_and_system_status_use_the_canonical_envelope;

mod bodyless_vendor_posts_reject_content;

mod backup_routes_authenticate_before_parsing_and_reject_get_bodies;

mod duplicate_role_tokens_never_resolve_to_a_role;

mod permission_and_query_errors_never_use_authentication_code;

mod storage_boundaries_return_cloudflare_errors_before_domain_dispatch;

mod d1_transfer_scope_media_and_query_contracts_fail_closed_before_authority;

mod authenticated_surface_fails_closed_without_product_authorities;

mod scope_matrix_is_minimal_and_explicit;
