use super::*;
use crate::health::HealthCoordinator;
use crate::http;
use crate::metrics::MetricsRegistry;
use axum::body::to_bytes;
use axum::http::Request;
use open_compute_core::SystemClock;
use open_compute_core::config::{MetricsConfig, StorageConfig};
use serde_json::{Value, json};
use tower::ServiceExt as _;

fn fixture() -> (tempfile::TempDir, Arc<PlatformStorage>, AccountId, Router) {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join("data");
    let storage = Arc::new(
        PlatformStorage::bootstrap(
            &StorageConfig {
                data_dir: root.clone(),
                master_key_file: root.join("keys/master.key"),
                master_key_env: None,
                sqlite_busy_timeout_ms: 5_000,
                free_space_soft_bytes: 1_073_741_824,
                free_space_hard_bytes: 268_435_456,
            },
            &SystemClock,
        )
        .unwrap(),
    );
    let account = storage.identity().default_account_id;
    let metrics =
        Arc::new(MetricsRegistry::new(&MetricsConfig::default(), "test", "workerd").unwrap());
    let state = HttpState::for_test(HealthCoordinator::new(), metrics, false, None)
        .with_search_api(SearchApiState::new(
            storage.clone(),
            ResourcePins::new(),
            5_000,
            Duration::from_millis(10),
        ));
    (temporary, storage, account, http::admin_router(state))
}

fn request(method: &str, uri: &str, body: &Value, key: Option<&str>) -> Request<Body> {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json");
    if let Some(key) = key {
        builder = builder.header("idempotency-key", key);
    }
    builder
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap()
}

async fn body(response: Response) -> (StatusCode, Value) {
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

#[tokio::test]
async fn operator_can_create_vector_metadata_and_ai_namespace_resources() {
    let (_temporary, storage, account, router) = fixture();
    let indexes = format!("/v1/accounts/{account}/vectorize/indexes");
    let (status, created) = body(
        router
            .clone()
            .oneshot(request(
                "POST",
                &indexes,
                &json!({"name":"documents","dimensions":32,"metric":"cosine"}),
                Some("vector-create"),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let vector_id: ResourceId = created["resourceId"].as_str().unwrap().parse().unwrap();
    let metadata = format!("{indexes}/{vector_id}/metadata-indexes");
    assert_eq!(
        router
            .clone()
            .oneshot(request(
                "POST",
                &metadata,
                &json!({"property_name":"language","property_type":"string"}),
                None,
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::CREATED
    );
    let record = VectorizeIndexRepository::new(storage.db())
        .get(account, vector_id)
        .unwrap();
    let path = VectorizePaths::open(storage.data_dir().root())
        .unwrap()
        .resolve_storage_key(&record.storage_key, account, vector_id)
        .unwrap();
    assert!(path.is_file());

    let namespaces = format!("/v1/accounts/{account}/ai-search/namespaces");
    let (status, created) = body(
        router
            .clone()
            .oneshot(request(
                "POST",
                &namespaces,
                &json!({"name":"knowledge"}),
                Some("namespace-create"),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let namespace_id: ResourceId = created["resourceId"].as_str().unwrap().parse().unwrap();
    assert!(
        AiSearchCatalog::new(storage.db())
            .get_namespace(account, namespace_id)
            .is_ok()
    );
}

#[tokio::test]
async fn operator_lists_gets_validates_and_deletes_search_resources() {
    let (_temporary, _storage, account, router) = fixture();
    let indexes = format!("/v1/accounts/{account}/vectorize/indexes");
    assert_eq!(
        body(
            router
                .clone()
                .oneshot(request("GET", &indexes, &Value::Null, None))
                .await
                .unwrap()
        )
        .await,
        (StatusCode::OK, json!({"indexes": []}))
    );
    for (payload, key) in [
        (
            json!({"name":"bad","dimensions":3,"metric":"cosine"}),
            Some("bad-dim"),
        ),
        (
            json!({"name":"bad","dimensions":32,"metric":"future"}),
            Some("bad-metric"),
        ),
        (
            json!({"name":"bad","dimensions":32,"metric":"cosine","quota_vectors":0}),
            Some("bad-quota"),
        ),
        (
            json!({"name":"bad","dimensions":32,"metric":"cosine"}),
            None,
        ),
    ] {
        assert_eq!(
            router
                .clone()
                .oneshot(request("POST", &indexes, &payload, key))
                .await
                .unwrap()
                .status(),
            StatusCode::BAD_REQUEST
        );
    }
    let (status, created) = body(
        router
            .clone()
            .oneshot(request(
                "POST",
                &indexes,
                &json!({"name":"vectors","dimensions":32,"metric":"dot-product"}),
                Some("vector-crud"),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let vector_id = created["resourceId"].as_str().unwrap();
    let vector = format!("{indexes}/{vector_id}");
    let (status, fetched) = body(
        router
            .clone()
            .oneshot(request("GET", &vector, &Value::Null, None))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(fetched["index"]["metric"], "dot-product");
    assert_eq!(
        body(
            router
                .clone()
                .oneshot(request("GET", &indexes, &Value::Null, None))
                .await
                .unwrap()
        )
        .await
        .1["indexes"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        router
            .clone()
            .oneshot(request("DELETE", &vector, &Value::Null, None))
            .await
            .unwrap()
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        router
            .clone()
            .oneshot(request(
                "DELETE",
                &vector,
                &Value::Null,
                Some("delete-vector"),
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::ACCEPTED
    );

    let namespaces = format!("/v1/accounts/{account}/ai-search/namespaces");
    assert_eq!(
        body(
            router
                .clone()
                .oneshot(request("GET", &namespaces, &Value::Null, None))
                .await
                .unwrap()
        )
        .await,
        (StatusCode::OK, json!({"namespaces": []}))
    );
    let (status, created) = body(
        router
            .clone()
            .oneshot(request(
                "POST",
                &namespaces,
                &json!({"name":"knowledge"}),
                Some("namespace-crud"),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let namespace = format!("{namespaces}/{}", created["resourceId"].as_str().unwrap());
    assert_eq!(
        router
            .clone()
            .oneshot(request("GET", &namespace, &Value::Null, None))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        body(
            router
                .clone()
                .oneshot(request("GET", &namespaces, &Value::Null, None))
                .await
                .unwrap()
        )
        .await
        .1["namespaces"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        router
            .oneshot(request(
                "DELETE",
                &namespace,
                &Value::Null,
                Some("delete-namespace"),
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::ACCEPTED
    );
}

#[test]
fn operator_error_mapping_covers_each_public_status_class() {
    for (code, status) in [
        (ErrorCode::ResourceNotFound, StatusCode::NOT_FOUND),
        (ErrorCode::ResourceNameConflict, StatusCode::CONFLICT),
        (ErrorCode::AdminAuthRequired, StatusCode::UNAUTHORIZED),
        (ErrorCode::LimitInvalid, StatusCode::BAD_REQUEST),
        (ErrorCode::QuotaExceeded, StatusCode::TOO_MANY_REQUESTS),
        (ErrorCode::DiskHardLimit, StatusCode::INSUFFICIENT_STORAGE),
        (
            ErrorCode::PlatformUnavailable,
            StatusCode::SERVICE_UNAVAILABLE,
        ),
        (ErrorCode::Internal, StatusCode::INTERNAL_SERVER_ERROR),
    ] {
        let error = PlatformError::new(code, "private detail");
        let response = error_response(&error, RequestId::generate());
        assert_eq!(response.status(), status);
        assert_eq!(
            response.extensions().get::<ProductErrorCode>().unwrap().0,
            code
        );
    }
}

#[tokio::test]
async fn operator_routes_fail_closed_without_authority_or_valid_ids() {
    let (_temporary, _storage, account, router) = fixture();
    assert_eq!(
        router
            .clone()
            .oneshot(request(
                "GET",
                "/v1/accounts/not-an-account/vectorize/indexes",
                &Value::Null,
                None,
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::BAD_REQUEST
    );
    let missing = format!(
        "/v1/accounts/{account}/vectorize/indexes/{}",
        ResourceId::generate()
    );
    assert_eq!(
        router
            .clone()
            .oneshot(request("GET", &missing, &Value::Null, None))
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
    let metrics =
        Arc::new(MetricsRegistry::new(&MetricsConfig::default(), "test", "workerd").unwrap());
    let unavailable = http::admin_router(HttpState::for_test(
        HealthCoordinator::new(),
        metrics.clone(),
        false,
        None,
    ));
    let indexes = format!("/v1/accounts/{account}/vectorize/indexes");
    assert_eq!(
        unavailable
            .oneshot(request("GET", &indexes, &Value::Null, None))
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
    let (_authenticated_temporary, authenticated_storage, _, _) = fixture();
    let authenticated = http::admin_router(
        HttpState::for_test(
            HealthCoordinator::new(),
            metrics,
            false,
            Some(open_compute_core::SecretString::new("operator-secret")),
        )
        .with_search_api(SearchApiState::new(
            authenticated_storage,
            ResourcePins::new(),
            5_000,
            Duration::from_millis(10),
        )),
    );
    assert_eq!(
        authenticated
            .oneshot(request("GET", &indexes, &Value::Null, None))
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
}
