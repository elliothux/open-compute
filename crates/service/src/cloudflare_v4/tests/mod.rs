use super::accounts::AccountAuthority;
use super::*;
use crate::health::HealthCoordinator;
use crate::http::{HttpState, REQUEST_ID_HEADER};
use crate::metrics::MetricsRegistry;
use axum::body::{Body, to_bytes};
use axum::http::{Method, Request, StatusCode, header};
use open_compute_core::clock::SystemClock;
use open_compute_core::config::{ArtifactsConfig, DataConfig, MetricsConfig};
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

fn artifacts_state() -> (tempfile::TempDir, HttpState, AccountAuthority) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("data");
    let storage = Arc::new(
        open_compute_storage::PlatformStorage::bootstrap(
            &DataConfig {
                path: root.clone(),
                master_key_file: root.join("keys/master.key"),
                master_key_env: None,
                sqlite_busy_timeout_ms: 5_000,
                free_space_soft_bytes: 1,
                free_space_hard_bytes: 1,
            },
            &SystemClock,
        )
        .unwrap(),
    );
    let authority = AccountAuthority::new(
        storage.identity().platform_id,
        storage.identity().default_account_id,
        storage.identity().created_at_ms,
    );
    let metrics = Arc::new(
        MetricsRegistry::new(&MetricsConfig::default(), "test", "workerd")
            .expect("metrics registry"),
    );
    let config = ArtifactsConfig {
        public_origin: "https://artifacts.example.test".to_owned(),
        ..ArtifactsConfig::default()
    };
    let api = crate::artifact_api::ArtifactApiState::new(Arc::clone(&storage), config).unwrap();
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
    .with_cloudflare_v4_account(authority.clone())
    .with_platform_storage(storage)
    .with_artifact_api(api);
    (temp, state, authority)
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

mod artifacts_management_contract;
