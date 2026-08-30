use super::*;
use crate::health::HealthCoordinator;
use crate::p3_3_test_support::RuntimeFeatureFixture;
use axum::body::{Body, to_bytes};
use axum::http::Request;
use open_compute_core::{
    AccountId, ErrorCode, ImagesConfig, MetricsConfig, PlatformError, ResponseCacheConfig,
    SecretString, WorkerId,
};
use open_compute_storage::{
    CacheBodyRef, CacheHeader, CacheIdentity, CacheMethod, CachePut, CacheStoredResponse,
    CacheSurface,
};
use open_compute_workers::{DeploymentImagesInput, DeploymentRuntimeFeatures};
use std::collections::BTreeMap;
use tower::ServiceExt as _;

#[tokio::test]
async fn operator_auth_stats_purge_gc_and_images_capacity_are_bounded() {
    let fixture = RuntimeFeatureFixture::create(DeploymentRuntimeFeatures {
        images: Some(DeploymentImagesInput {
            binding: "IMAGES".to_owned(),
        }),
        ..DeploymentRuntimeFeatures::default()
    })
    .await;
    let cache = Arc::new(
        CacheManager::open(
            fixture.storage.data_dir().root(),
            ResponseCacheConfig::default(),
        )
        .unwrap(),
    );
    let engine = cache.engine(fixture.account, fixture.worker, 1).unwrap();
    let identity = CacheIdentity {
        account_id: fixture.account,
        worker_id: fixture.worker,
        surface: CacheSurface::CacheApiDefault,
        entrypoint: None,
        version_scope: "shared".to_owned(),
        cache_name: None,
        canonical_url: "https://example.test/operator".to_owned(),
        method: CacheMethod::Get,
    };
    let fence = engine.prepare_put(&identity).unwrap();
    engine
        .put(&CachePut {
            identity,
            request_headers: BTreeMap::new(),
            response: CacheStoredResponse {
                status: 200,
                headers: vec![CacheHeader {
                    name: "cache-control".to_owned(),
                    value: "max-age=60".to_owned(),
                }],
                body: CacheBodyRef {
                    sha256: "11".repeat(32),
                    size: 4,
                },
                vary: Vec::new(),
                tags: Vec::new(),
                fresh_until_ms: 10_000,
                stale_while_revalidate_until_ms: 10_000,
                stale_if_error_until_ms: 10_000,
                generation: 1,
            },
            expected_fence_generation: fence,
            refresh_token: None,
            now_ms: 2,
        })
        .unwrap();
    let metrics =
        Arc::new(MetricsRegistry::new(&MetricsConfig::default(), "test", "workerd").unwrap());
    let images = Arc::new(ImageBindingService::new(
        fixture.storage.clone(),
        ImagesConfig::default(),
    ));
    let api = CacheImagesApiState::new(
        fixture.storage.clone(),
        cache,
        images,
        fixture.artifacts.clone(),
        WorkersConfig::default(),
        Arc::new(SnapshotPins::empty()),
        metrics.clone(),
    );
    assert!(format!("{api:?}").starts_with("CacheImagesApiState"));
    let state = HttpState::for_test(
        HealthCoordinator::new(),
        metrics,
        true,
        Some(SecretString::new("operator-token")),
    )
    .with_cache_images_api(api);
    let app = crate::http::admin_router(state);
    let base = format!(
        "/v1/operator/accounts/{}/workers/{}/cache",
        fixture.account, fixture.worker,
    );

    let denied = app
        .clone()
        .oneshot(Request::builder().uri(&base).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);
    let invalid = app
        .clone()
        .oneshot(authorized(
            "GET",
            "/v1/operator/accounts/not-an-account/workers/not-a-worker/cache",
        ))
        .await
        .unwrap();
    assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);
    let missing = app
        .clone()
        .oneshot(authorized(
            "GET",
            &format!(
                "/v1/operator/accounts/{}/workers/{}/cache",
                AccountId::generate(),
                WorkerId::generate(),
            ),
        ))
        .await
        .unwrap();
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    let stats = app.clone().oneshot(authorized("GET", &base)).await.unwrap();
    assert_eq!(stats.status(), StatusCode::OK);
    let body: serde_json::Value =
        serde_json::from_slice(&to_bytes(stats.into_body(), 4096).await.unwrap()).unwrap();
    assert_eq!(body["entries"], 1);
    assert_eq!(body["bodyBytes"], 4);
    let purged = app
        .clone()
        .oneshot(authorized("POST", &format!("{base}/purge")))
        .await
        .unwrap();
    assert_eq!(purged.status(), StatusCode::OK);
    let capacity = app
        .clone()
        .oneshot(authorized("GET", "/v1/operator/images/capacity"))
        .await
        .unwrap();
    assert_eq!(capacity.status(), StatusCode::OK);
    let gc = app
        .oneshot(authorized("POST", "/v1/operator/cache/gc"))
        .await
        .unwrap();
    assert_eq!(gc.status(), StatusCode::OK);
}

#[tokio::test]
async fn operator_routes_are_unavailable_without_the_composed_p3_authority() {
    let metrics =
        Arc::new(MetricsRegistry::new(&MetricsConfig::default(), "test", "workerd").unwrap());
    let state = HttpState::for_test(
        HealthCoordinator::new(),
        metrics,
        true,
        Some(SecretString::new("operator-token")),
    );
    let app = crate::http::admin_router(state);
    for (method, path) in [
        (
            "GET",
            "/v1/operator/accounts/not-an-account/workers/not-a-worker/cache",
        ),
        (
            "POST",
            "/v1/operator/accounts/not-an-account/workers/not-a-worker/cache/purge",
        ),
        ("POST", "/v1/operator/cache/gc"),
        ("GET", "/v1/operator/images/capacity"),
    ] {
        let response = app.clone().oneshot(authorized(method, path)).await.unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE, "{path}");
    }
}

#[test]
fn operator_error_mapping_is_stable_and_sanitized() {
    for (code, status) in [
        (ErrorCode::AccountNotFound, StatusCode::NOT_FOUND),
        (ErrorCode::WorkerNotFound, StatusCode::NOT_FOUND),
        (ErrorCode::WorkerDeleted, StatusCode::GONE),
        (ErrorCode::CacheCorrupt, StatusCode::INTERNAL_SERVER_ERROR),
        (ErrorCode::CacheUnavailable, StatusCode::SERVICE_UNAVAILABLE),
        (
            ErrorCode::ArtifactUnavailable,
            StatusCode::SERVICE_UNAVAILABLE,
        ),
        (ErrorCode::CacheProtocolError, StatusCode::BAD_REQUEST),
    ] {
        let response = operator_error(&PlatformError::new(code, "secret detail"));
        assert_eq!(response.status(), status);
    }
    assert_eq!(invalid().code(), ErrorCode::CacheProtocolError);
    assert!(now_ms() > 0);
}

fn authorized(method: &str, uri: &str) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", "Bearer operator-token")
        .body(Body::empty())
        .unwrap()
}
