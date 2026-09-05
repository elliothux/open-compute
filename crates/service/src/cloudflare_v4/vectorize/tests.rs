use super::*;
use crate::cloudflare_v4::accounts::AccountAuthority;
use crate::cloudflare_v4::{router as v4_router, storage_router};
use crate::health::HealthCoordinator;
use crate::http::{HttpState, REQUEST_ID_HEADER};
use crate::metrics::MetricsRegistry;
use crate::search_api::SearchApiState;
use crate::vectorize_coordinator::VectorizeCoordinator;
use axum::body::{Body, to_bytes};
use axum::http::{HeaderValue, Request, StatusCode, header};
use open_compute_core::config::{DataConfig, MetricsConfig};
use open_compute_core::{SecretString, SystemClock};
use open_compute_storage::PlatformStorage;
use open_compute_workers::ResourcePins;
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;
use tower::ServiceExt as _;

struct Fixture {
    _temp: tempfile::TempDir,
    storage: Arc<PlatformStorage>,
    state: HttpState,
    account: String,
    pins: ResourcePins,
}

fn fixture() -> Fixture {
    let temp = tempfile::tempdir().expect("temporary data directory");
    let root = temp.path().join("data");
    let storage = Arc::new(
        PlatformStorage::bootstrap(
            &DataConfig {
                path: root.clone(),
                master_key_file: root.join("keys/master.key"),
                master_key_env: None,
                sqlite_busy_timeout_ms: 5_000,
                free_space_soft_bytes: 1_073_741_824,
                free_space_hard_bytes: 1,
            },
            &SystemClock,
        )
        .expect("platform storage"),
    );
    let authority = AccountAuthority::new(
        storage.identity().platform_id,
        storage.identity().default_account_id,
        storage.identity().created_at_ms,
    );
    let account = authority.public_id().to_owned();
    let metrics = Arc::new(
        MetricsRegistry::new(&MetricsConfig::default(), "test", "workerd")
            .expect("metrics registry"),
    );
    let pins = ResourcePins::new();
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
    .with_cloudflare_v4_account(authority)
    .with_search_api(SearchApiState::new(
        storage.clone(),
        pins.clone(),
        5_000,
        Duration::from_secs(1),
    ));
    Fixture {
        _temp: temp,
        storage,
        state,
        account,
        pins,
    }
}

fn app(state: HttpState) -> Router {
    v4_router(state.clone(), storage_router()).with_state(state)
}

fn app_with_default_limit_probe(state: HttpState) -> Router {
    v4_router(state.clone(), storage_router())
        .route("/__test/default-multipart-limit", post(default_limit_probe))
        .with_state(state)
}

async fn default_limit_probe(mut multipart: Multipart) -> Result<(), MultipartError> {
    while let Some(field) = multipart.next_field().await? {
        let _ = field.bytes().await?;
    }
    Ok(())
}

async fn send(
    fixture: &Fixture,
    method: &str,
    path: &str,
    token: &str,
    content_type: Option<&str>,
    body: impl Into<Body>,
) -> Response {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header(header::AUTHORIZATION, format!("Bearer {token}"));
    if let Some(content_type) = content_type {
        request = request.header(header::CONTENT_TYPE, content_type);
    }
    app(fixture.state.clone())
        .oneshot(request.body(body.into()).expect("request"))
        .await
        .expect("response")
}

async fn body(response: Response) -> Value {
    assert!(response.headers().contains_key(REQUEST_ID_HEADER));
    serde_json::from_slice(
        &to_bytes(response.into_body(), 1024 * 1024)
            .await
            .expect("bounded response"),
    )
    .expect("JSON response")
}

#[tokio::test]
async fn multipart_uses_mime_boundary_parsing_and_the_explicit_total_limit() {
    let fixture = fixture();
    let root = format!("/accounts/{}/vectorize/v2/indexes", fixture.account);
    let create = send(
        &fixture,
        "POST",
        &root,
        "deployer-token",
        Some("application/json"),
        json!({"name":"large-vectors","config":{"dimensions":1,"metric":"cosine"}}).to_string(),
    )
    .await;
    assert_eq!(create.status(), StatusCode::OK);

    let boundary = "quoted-vectorize-boundary";
    let multipart = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"vectors\"; filename=\"vectors.ndjson\"\r\nContent-Type: application/x-ndjson\r\n\r\n{}\n{}\n\r\n--{boundary}--\r\n",
        json!({"id":"kept","values":[1]}),
        "x".repeat(2 * 1024 * 1024),
    );
    assert!(multipart.len() > 2 * 1024 * 1024);
    let insert = send(
        &fixture,
        "POST",
        &format!("{root}/large-vectors/insert?unparsable-behavior=discard"),
        "deployer-token",
        Some(&format!(
            "multipart/form-data;charset=utf-8;boundary=\"{boundary}\""
        )),
        multipart.clone(),
    )
    .await;
    assert_eq!(insert.status(), StatusCode::OK);
    assert!(body(insert).await["result"]["mutationId"].is_string());

    let mut duplicate = Request::builder()
        .method("POST")
        .uri(format!("{root}/large-vectors/insert"))
        .header(header::AUTHORIZATION, "Bearer deployer-token")
        .body(Body::from(multipart))
        .expect("request");
    duplicate.headers_mut().append(
        header::CONTENT_TYPE,
        HeaderValue::from_static("multipart/form-data;boundary=quoted-vectorize-boundary"),
    );
    duplicate.headers_mut().append(
        header::CONTENT_TYPE,
        HeaderValue::from_static("multipart/form-data;boundary=quoted-vectorize-boundary"),
    );
    let duplicate = app(fixture.state.clone())
        .oneshot(duplicate)
        .await
        .expect("response");
    assert_eq!(duplicate.status(), StatusCode::BAD_REQUEST);

    let invalid = send(
        &fixture,
        "POST",
        &format!("{root}/large-vectors/insert"),
        "deployer-token",
        Some("multipart/form-data;boundary=\"unterminated"),
        Body::empty(),
    )
    .await;
    assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);

    let oversized_raw = send(
        &fixture,
        "POST",
        &format!("{root}/large-vectors/insert"),
        "deployer-token",
        Some("application/x-ndjson"),
        vec![b' '; MAX_NDJSON_BODY + 1],
    )
    .await;
    assert_eq!(oversized_raw.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(body(oversized_raw).await["errors"][0]["code"], 10_027);

    let oversized_boundary = "oversized-vectorize-boundary";
    let mut oversized_multipart = format!(
        "--{oversized_boundary}\r\nContent-Disposition: form-data; name=\"vectors\"; filename=\"vectors.ndjson\"\r\nContent-Type: application/x-ndjson\r\n\r\n"
    )
    .into_bytes();
    oversized_multipart.resize(MAX_NDJSON_BODY + 1, b' ');
    let oversized_multipart = send(
        &fixture,
        "POST",
        &format!("{root}/large-vectors/insert"),
        "deployer-token",
        Some(&format!(
            "multipart/form-data; boundary={oversized_boundary}"
        )),
        oversized_multipart,
    )
    .await;
    assert_eq!(oversized_multipart.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(body(oversized_multipart).await["errors"][0]["code"], 10_027);
}

#[tokio::test]
async fn vector_mutation_body_limit_does_not_expand_sibling_routes() {
    let fixture = fixture();
    let boundary = "default-limit-probe";
    let multipart = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"probe\"\r\n\r\n{}\r\n--{boundary}--\r\n",
        "x".repeat(2 * 1024 * 1024),
    );
    let response = app_with_default_limit_probe(fixture.state.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/__test/default-multipart-limit")
                .header(
                    header::CONTENT_TYPE,
                    format!("multipart/form-data; boundary={boundary}"),
                )
                .body(Body::from(multipart))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
}

#[tokio::test]
async fn all_vectorize_routes_use_the_official_boundary_and_durable_engine() {
    let fixture = fixture();
    let root = format!("/accounts/{}/vectorize/v2/indexes", fixture.account);
    let create = send(
        &fixture,
        "POST",
        &root,
        "deployer-token",
        Some("application/json; charset=utf-8;"),
        json!({"name":"vectors","config":{"dimensions":1,"metric":"cosine"},"description":"official"}).to_string(),
    )
    .await;
    assert_eq!(create.status(), StatusCode::OK);
    let create = body(create).await;
    assert_eq!(create["result"]["name"], "vectors");
    assert_eq!(create["result"]["description"], "official");

    let duplicate_charset = send(
        &fixture,
        "POST",
        &root,
        "deployer-token",
        Some("application/json;charset=utf-8;charset=utf-8"),
        json!({"name":"invalid","config":{"dimensions":32,"metric":"cosine"}}).to_string(),
    )
    .await;
    assert_eq!(duplicate_charset.status(), StatusCode::BAD_REQUEST);

    let list = send(&fixture, "GET", &root, "read-token", None, Body::empty()).await;
    assert_eq!(body(list).await["result"][0]["name"], "vectors");
    let index = format!("{root}/vectors");
    let get = send(&fixture, "GET", &index, "read-token", None, Body::empty()).await;
    assert_eq!(body(get).await["result"]["config"]["dimensions"], 1);

    let values = vec![1.0];
    let boundary = "fixed-wrangler-vectorize-boundary";
    let multipart = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"vectors\"; filename=\"vectors.ndjson\"\r\nContent-Type: application/x-ndjson\r\n\r\n{}\n\r\n--{boundary}--\r\n",
        json!({"id":"first","values":values,"metadata":{"kind":"one"}})
    );
    let insert = send(
        &fixture,
        "POST",
        &format!("{index}/insert?unparsable-behavior=error"),
        "deployer-token",
        Some(&format!("multipart/form-data; boundary={boundary}")),
        multipart,
    )
    .await;
    assert!(body(insert).await["result"]["mutationId"].is_string());
    let extra_part = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"vectors\"; filename=\"vectors.ndjson\"\r\nContent-Type: application/x-ndjson\r\n\r\n{}\n\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"extra\"\r\n\r\nrejected\r\n--{boundary}--\r\n",
        json!({"id":"rejected","values":values})
    );
    let rejected_extra_part = send(
        &fixture,
        "POST",
        &format!("{index}/insert"),
        "deployer-token",
        Some(&format!("multipart/form-data; boundary={boundary}")),
        extra_part,
    )
    .await;
    assert_eq!(rejected_extra_part.status(), StatusCode::BAD_REQUEST);
    let upsert = send(
        &fixture,
        "POST",
        &format!("{index}/upsert"),
        "deployer-token",
        Some("application/x-ndjson"),
        format!("{}\n", json!({"id":"second","values":values})),
    )
    .await;
    assert!(body(upsert).await["result"]["mutationId"].is_string());
    let coordinator = VectorizeCoordinator::new(fixture.storage.clone(), fixture.pins.clone());
    assert_eq!(coordinator.drain_once().expect("first drain").applied, 1);
    assert_eq!(coordinator.drain_once().expect("second drain").applied, 1);

    let get_ids = send(
        &fixture,
        "POST",
        &format!("{index}/get_by_ids"),
        "read-token",
        Some("application/json"),
        json!({"ids":["second","first"]}).to_string(),
    )
    .await;
    assert_eq!(body(get_ids).await["result"].as_array().unwrap().len(), 2);
    let query = send(
        &fixture,
        "POST",
        &format!("{index}/query"),
        "read-token",
        Some("application/json"),
        json!({"vector":values,"topK":1,"returnValues":true}).to_string(),
    )
    .await;
    assert_eq!(body(query).await["result"]["count"], 1);
    let info = send(
        &fixture,
        "GET",
        &format!("{index}/info"),
        "read-token",
        None,
        Body::empty(),
    )
    .await;
    assert_eq!(body(info).await["result"]["vectorCount"], 2);

    let first_page = send(
        &fixture,
        "GET",
        &format!("{index}/list?count=1"),
        "read-token",
        None,
        Body::empty(),
    )
    .await;
    let first_page = body(first_page).await;
    assert_eq!(first_page["result"]["isTruncated"], true);
    let cursor = first_page["result"]["nextCursor"].as_str().unwrap();
    let second_page = send(
        &fixture,
        "GET",
        &format!("{index}/list?count=1&cursor={cursor}"),
        "read-token",
        None,
        Body::empty(),
    )
    .await;
    let second_page = body(second_page).await;
    assert_eq!(second_page["result"]["count"], 1);
    assert_eq!(second_page["result"]["totalCount"], 2);
    let wrong_binding = send(
        &fixture,
        "GET",
        &format!("{index}/list?count=2&cursor={cursor}"),
        "read-token",
        None,
        Body::empty(),
    )
    .await;
    assert_eq!(wrong_binding.status(), StatusCode::BAD_REQUEST);

    let metadata = format!("{index}/metadata_index");
    let create_metadata = send(
        &fixture,
        "POST",
        &format!("{metadata}/create"),
        "deployer-token",
        Some("application/json"),
        json!({"propertyName":"kind","indexType":"string"}).to_string(),
    )
    .await;
    assert!(body(create_metadata).await["result"]["mutationId"].is_string());
    let list_metadata = send(
        &fixture,
        "GET",
        &format!("{metadata}/list"),
        "read-token",
        None,
        Body::empty(),
    )
    .await;
    assert_eq!(
        body(list_metadata).await["result"]["metadataIndexes"][0]["propertyName"],
        "kind"
    );
    let delete_metadata = send(
        &fixture,
        "POST",
        &format!("{metadata}/delete"),
        "deployer-token",
        Some("application/json"),
        json!({"propertyName":"kind"}).to_string(),
    )
    .await;
    assert!(body(delete_metadata).await["result"]["mutationId"].is_string());

    let delete_ids = send(
        &fixture,
        "POST",
        &format!("{index}/delete_by_ids"),
        "deployer-token",
        Some("application/json"),
        json!({"ids":["first"]}).to_string(),
    )
    .await;
    assert!(body(delete_ids).await["result"]["mutationId"].is_string());
    assert_eq!(coordinator.drain_once().expect("delete drain").applied, 1);

    let delete = send(
        &fixture,
        "DELETE",
        &index,
        "deployer-token",
        None,
        Body::empty(),
    )
    .await;
    assert_eq!(delete.status(), StatusCode::OK);
    assert!(body(delete).await["result"].is_null());
}
