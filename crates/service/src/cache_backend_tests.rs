use super::*;
use crate::p3_3_test_support::RuntimeFeatureFixture;
use axum::http::Request;
use futures::StreamExt as _;
use open_compute_storage::CacheHeader;
use open_compute_workers::{
    DeploymentCacheInput, DeploymentCachePolicyInput, DeploymentRuntimeFeatures,
};
use std::collections::HashSet;

async fn fixture() -> (RuntimeFeatureFixture, CacheBindingService) {
    let fixture = RuntimeFeatureFixture::create(DeploymentRuntimeFeatures {
        cache: DeploymentCacheInput {
            default: DeploymentCachePolicyInput {
                enabled: true,
                cross_version_cache: false,
            },
            entrypoints: BTreeMap::new(),
        },
        images: None,
        ai: None,
        version_metadata: None,
    })
    .await;
    let service = CacheBindingService::new(
        fixture.storage.clone(),
        fixture.artifacts.clone(),
        fixture.artifact_cache.clone(),
        ResponseCacheConfig::default(),
    )
    .unwrap();
    (fixture, service)
}

fn request(fixture: &RuntimeFeatureFixture, path: &str, body: Body) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(path)
        .header(ACCOUNT_HEADER, fixture.account.to_string())
        .header(WORKER_HEADER, fixture.worker.to_string())
        .header(DEPLOYMENT_HEADER, fixture.deployment.to_string())
        .header(ENTRYPOINT_HEADER, "default")
        .header(DESCRIPTOR_HEADER, &fixture.descriptor_sha256)
        .header(ENABLED_HEADER, "true")
        .header(CROSS_VERSION_HEADER, "false")
        .body(body)
        .unwrap()
}

fn cache_request(namespace: &str, name: Option<&str>, headers: &serde_json::Value) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "namespace": namespace,
        "name": name,
        "url": "https://EXAMPLE.test:443/page?a=1&a=2",
        "method": "GET",
        "headers": headers,
    }))
    .unwrap()
}

fn put_frame(
    namespace: &str,
    name: Option<&str>,
    request_headers: &serde_json::Value,
    body: &[u8],
    fence: Option<(&str, Option<&str>)>,
) -> Body {
    put_frame_with_status(namespace, name, request_headers, body, fence, 200)
}

fn put_frame_with_status(
    namespace: &str,
    name: Option<&str>,
    request_headers: &serde_json::Value,
    body: &[u8],
    fence: Option<(&str, Option<&str>)>,
    status: u16,
) -> Body {
    let mut metadata = serde_json::json!({
        "namespace": namespace,
        "name": name,
        "url": "https://EXAMPLE.test:443/page?a=1&a=2",
        "method": "GET",
        "headers": request_headers,
        "status": status,
        "responseHeaders": [
            ["cache-control", "max-age=60, stale-while-revalidate=30, stale-if-error=60"],
            ["cache-tag", "News, product"],
            ["content-type", "text/plain"],
            ["etag", "\"v1\""],
            ["last-modified", "Sun, 06 Nov 1994 08:49:37 GMT"],
            ["vary", "accept-language"]
        ]
    });
    if let Some((generation, token)) = fence {
        metadata["expectedFenceGeneration"] = serde_json::json!(generation);
        if let Some(token) = token {
            metadata["refreshToken"] = serde_json::json!(token);
        }
    }
    let metadata = serde_json::to_vec(&metadata).unwrap();
    let mut bytes = Vec::with_capacity(4 + metadata.len() + body.len());
    bytes.extend_from_slice(&u32::try_from(metadata.len()).unwrap().to_be_bytes());
    bytes.extend_from_slice(&metadata);
    bytes.extend_from_slice(body);
    Body::from(bytes)
}

fn policy_put_frame(
    request_headers: &serde_json::Value,
    response_headers: &serde_json::Value,
) -> Body {
    let metadata = serde_json::to_vec(&serde_json::json!({
        "namespace": "default",
        "name": null,
        "url": "https://example.test/policy",
        "method": "GET",
        "headers": request_headers,
        "status": 200,
        "responseHeaders": response_headers,
    }))
    .unwrap();
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&u32::try_from(metadata.len()).unwrap().to_be_bytes());
    bytes.extend_from_slice(&metadata);
    bytes.extend_from_slice(b"policy");
    Body::from(bytes)
}

async fn response_bytes(response: Response) -> Vec<u8> {
    to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap()
        .to_vec()
}

async fn wait_for_artifact_request(fixture: &RuntimeFeatureFixture, digest: &str, method: &str) {
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if fixture._mock.recorded().iter().any(|request| {
                request.method == method
                    && request.path.contains("/artifacts/v1/sha256/")
                    && request.path.contains(&digest[..2])
                    && request.path.contains(&digest[2..])
            }) {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("cache artifact request admission deadline");
}

#[tokio::test]
async fn cache_api_wire_covers_namespaces_vary_conditions_ranges_and_delete() {
    let (fixture, service) = fixture().await;
    let request_headers = serde_json::json!([["accept-language", "en"]]);
    let put = service
        .handle(request(
            &fixture,
            "/internal/cache/v1/put",
            put_frame("default", None, &request_headers, b"abcdef", None),
        ))
        .await;
    assert_eq!(put.status(), StatusCode::NO_CONTENT);

    let hit = service
        .handle(request(
            &fixture,
            "/internal/cache/v1/match",
            Body::from(cache_request("default", None, &request_headers)),
        ))
        .await;
    assert_eq!(hit.status(), StatusCode::OK);
    assert_eq!(hit.headers()["cf-cache-status"], "HIT");
    assert!(!hit.headers().contains_key("cache-tag"));
    assert_eq!(response_bytes(hit).await, b"abcdef");

    let variant_miss = service
        .handle(request(
            &fixture,
            "/internal/cache/v1/match",
            Body::from(cache_request(
                "default",
                None,
                &serde_json::json!([["accept-language", "fr"]]),
            )),
        ))
        .await;
    assert_eq!(variant_miss.status(), StatusCode::NO_CONTENT);

    let conditional = service
        .handle(request(
            &fixture,
            "/internal/cache/v1/match",
            Body::from(cache_request(
                "default",
                None,
                &serde_json::json!([["accept-language", "en"], ["if-none-match", "W/\"v1\""]]),
            )),
        ))
        .await;
    assert_eq!(conditional.status(), StatusCode::NOT_MODIFIED);
    assert!(response_bytes(conditional).await.is_empty());

    let range = service
        .handle(request(
            &fixture,
            "/internal/cache/v1/match",
            Body::from(cache_request(
                "default",
                None,
                &serde_json::json!([["accept-language", "en"], ["range", "bytes=1-3"]]),
            )),
        ))
        .await;
    assert_eq!(range.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(range.headers()[header::CONTENT_RANGE], "bytes 1-3/6");
    assert_eq!(response_bytes(range).await, b"bcd");

    let unsatisfied = service
        .handle(request(
            &fixture,
            "/internal/cache/v1/match",
            Body::from(cache_request(
                "default",
                None,
                &serde_json::json!([["accept-language", "en"], ["range", "bytes=99-"]]),
            )),
        ))
        .await;
    assert_eq!(unsatisfied.status(), StatusCode::RANGE_NOT_SATISFIABLE);
    assert_eq!(unsatisfied.headers()[header::CONTENT_RANGE], "bytes */6");

    let named = service
        .handle(request(
            &fixture,
            "/internal/cache/v1/put",
            put_frame("named", Some("pages"), &request_headers, b"named", None),
        ))
        .await;
    assert_eq!(named.status(), StatusCode::NO_CONTENT);
    let named_hit = service
        .handle(request(
            &fixture,
            "/internal/cache/v1/match",
            Body::from(cache_request("named", Some("pages"), &request_headers)),
        ))
        .await;
    assert_eq!(response_bytes(named_hit).await, b"named");

    let deleted = service
        .handle(request(
            &fixture,
            "/internal/cache/v1/delete",
            Body::from(cache_request("default", None, &request_headers)),
        ))
        .await;
    assert_eq!(deleted.status(), StatusCode::OK);
    assert_eq!(response_bytes(deleted).await, br#"{"deleted":true}"#);
    let miss = service
        .handle(request(
            &fixture,
            "/internal/cache/v1/match",
            Body::from(cache_request("default", None, &request_headers)),
        ))
        .await;
    assert_eq!(miss.status(), StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn automatic_cache_wire_fences_purge_and_rejects_forged_authority() {
    let (fixture, service) = fixture().await;
    let request_headers = serde_json::json!([["accept-language", "en"]]);
    let lookup = service
        .handle(request(
            &fixture,
            "/internal/cache/v1/match",
            Body::from(cache_request("automatic", None, &request_headers)),
        ))
        .await;
    assert_eq!(lookup.status(), StatusCode::NO_CONTENT);
    let fence = lookup.headers()["x-open-compute-cache-fence"]
        .to_str()
        .unwrap()
        .to_owned();
    let put = service
        .handle(request(
            &fixture,
            "/internal/cache/v1/put",
            put_frame(
                "automatic",
                None,
                &request_headers,
                b"automatic",
                Some((&fence, None)),
            ),
        ))
        .await;
    assert_eq!(put.status(), StatusCode::NO_CONTENT);

    let purge = service
        .handle(request(
            &fixture,
            "/internal/cache/v1/purge",
            Body::from(br#"{"tags":["news"]}"#.as_slice()),
        ))
        .await;
    assert_eq!(purge.status(), StatusCode::OK);
    assert_eq!(
        response_bytes(purge).await,
        br#"{"deleted":1,"success":true}"#
    );

    let late = service
        .handle(request(
            &fixture,
            "/internal/cache/v1/put",
            put_frame(
                "automatic",
                None,
                &request_headers,
                b"late",
                Some((&fence, None)),
            ),
        ))
        .await;
    assert_eq!(late.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        late.headers()[ERROR_HEADER],
        ErrorCode::CacheResultUnknown.as_str()
    );

    let mut forged = request(
        &fixture,
        "/internal/cache/v1/match",
        Body::from(cache_request("automatic", None, &serde_json::json!([]))),
    );
    forged.headers_mut().insert(
        HeaderName::from_static(DESCRIPTOR_HEADER),
        HeaderValue::from_static("00"),
    );
    let denied = service.handle(forged).await;
    assert_eq!(denied.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        denied.headers()[ERROR_HEADER],
        ErrorCode::CacheProtocolError.as_str()
    );
}

#[tokio::test]
async fn cache_body_integrity_failure_is_not_reported_as_fail_open_availability() {
    let (fixture, service) = fixture().await;
    let request_headers = serde_json::json!([]);
    let put = service
        .handle(request(
            &fixture,
            "/internal/cache/v1/put",
            put_frame("default", None, &request_headers, b"integrity", None),
        ))
        .await;
    assert_eq!(put.status(), StatusCode::NO_CONTENT);
    let digest = hex::encode(Sha256::digest(b"integrity"));
    let suffix = format!("{}/{}", &digest[..2], &digest[2..]);
    let key = fixture
        ._mock
        .keys()
        .into_iter()
        .find(|key| key.ends_with(&suffix))
        .unwrap();
    fixture._mock.corrupt_body(&key);

    let response = service
        .handle(request(
            &fixture,
            "/internal/cache/v1/match",
            Body::from(cache_request("default", None, &request_headers)),
        ))
        .await;
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(
        response.headers()[ERROR_HEADER],
        ErrorCode::CacheCorrupt.as_str()
    );
}

#[tokio::test]
async fn cache_request_deadline_cancels_and_removes_partial_staging() {
    let (fixture, _) = fixture().await;
    let service = CacheBindingService::new(
        fixture.storage.clone(),
        fixture.artifacts.clone(),
        fixture.artifact_cache.clone(),
        ResponseCacheConfig {
            request_timeout_ms: 20,
            ..ResponseCacheConfig::default()
        },
    )
    .unwrap();
    let initial = to_bytes(
        put_frame("default", None, &serde_json::json!([]), b"partial", None),
        usize::MAX,
    )
    .await
    .unwrap();
    let body = Body::from_stream(
        stream::once(async move { Ok::<_, std::convert::Infallible>(initial) })
            .chain(stream::pending()),
    );
    let response = service
        .handle(request(&fixture, "/internal/cache/v1/put", body))
        .await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        response.headers()[ERROR_HEADER],
        ErrorCode::CacheUnavailable.as_str()
    );
    assert_eq!(
        std::fs::read_dir(fixture.storage.data_dir().deployment_staging_dir())
            .unwrap()
            .count(),
        0
    );
}

#[tokio::test]
async fn cache_put_policy_ignores_extensions_but_rejects_private_authority() {
    let (fixture, service) = fixture().await;
    let default_ttl = service
        .handle(request(
            &fixture,
            "/internal/cache/v1/put",
            policy_put_frame(&serde_json::json!([]), &serde_json::json!([])),
        ))
        .await;
    assert_eq!(default_ttl.status(), StatusCode::NO_CONTENT);

    let accepted = service
        .handle(request(
            &fixture,
            "/internal/cache/v1/put",
            policy_put_frame(
                &serde_json::json!([]),
                &serde_json::json!([["cache-control", "max-age=60, immutable=opaque"]]),
            ),
        ))
        .await;
    assert_eq!(accepted.status(), StatusCode::NO_CONTENT);

    for (request_headers, response_headers) in [
        (
            serde_json::json!([]),
            serde_json::json!([["cache-control", "private=\"etag\", max-age=60"]]),
        ),
        (
            serde_json::json!([["authorization", "Bearer secret"]]),
            serde_json::json!([["cache-control", "max-age=60"]]),
        ),
        (
            serde_json::json!([]),
            serde_json::json!([
                ["cache-control", "max-age=60"],
                ["set-cookie", "secret=value"]
            ]),
        ),
    ] {
        let rejected = service
            .handle(request(
                &fixture,
                "/internal/cache/v1/put",
                policy_put_frame(&request_headers, &response_headers),
            ))
            .await;
        assert_eq!(rejected.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(
            rejected.headers()[ERROR_HEADER],
            ErrorCode::CachePutRejected.as_str()
        );
    }
}

#[tokio::test]
async fn cache_put_rejects_non_fetch_response_statuses() {
    let (fixture, service) = fixture().await;
    for status in [199, 206, 600] {
        let rejected = service
            .handle(request(
                &fixture,
                "/internal/cache/v1/put",
                put_frame_with_status(
                    "default",
                    None,
                    &serde_json::json!([]),
                    b"status",
                    None,
                    status,
                ),
            ))
            .await;
        assert_eq!(rejected.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(
            rejected.headers()[ERROR_HEADER],
            ErrorCode::CachePutRejected.as_str()
        );
    }
}

#[tokio::test]
async fn cache_body_reference_commit_fences_concurrent_artifact_gc() {
    let (fixture, service) = fixture().await;
    fixture._mock.clear_recorded();
    fixture._mock.synchronize_next_heads(2);
    let body = b"gc-fenced-body";
    let digest = hex::encode(Sha256::digest(body));
    let artifact = ArtifactRef::new(ARTIFACT_KEY_VERSION, &digest, body.len() as u64).unwrap();
    let put_request = request(
        &fixture,
        "/internal/cache/v1/put",
        put_frame("default", None, &serde_json::json!([]), body, None),
    );
    let put_service = service.clone();
    let put = tokio::spawn(async move { put_service.handle(put_request).await });

    wait_for_artifact_request(&fixture, &digest, "HEAD").await;

    let gc_store = fixture.artifacts.clone();
    let (acquired_tx, mut acquired_rx) = tokio::sync::oneshot::channel();
    let gc = tokio::spawn(async move {
        let _fence = gc_store.fence_deployment_gc().await;
        let _ = acquired_tx.send(());
    });
    assert!(
        tokio::time::timeout(Duration::from_millis(20), &mut acquired_rx)
            .await
            .is_err(),
        "artifact GC must wait through cache metadata commit"
    );

    let _ = fixture.artifacts.head(&artifact).await;
    let response = tokio::time::timeout(Duration::from_secs(5), put)
        .await
        .expect("cache put deadline")
        .expect("cache put task");
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    tokio::time::timeout(Duration::from_secs(1), &mut acquired_rx)
        .await
        .expect("artifact GC fence deadline")
        .expect("artifact GC fence sender");
    gc.await.expect("artifact GC fence task");
}

#[tokio::test]
async fn cache_hit_handoff_to_local_stream_pin_fences_remote_gc() {
    let (fixture, service) = fixture().await;
    let body = b"stream-survives-purge-stream-survives-purge-stream-survives-purge";
    let digest = hex::encode(Sha256::digest(body));
    let stored = service
        .handle(request(
            &fixture,
            "/internal/cache/v1/put",
            put_frame("default", None, &serde_json::json!([]), body, None),
        ))
        .await;
    assert_eq!(stored.status(), StatusCode::NO_CONTENT);

    fixture._mock.clear_recorded();
    fixture._mock.set_get_chunking(1, Duration::from_millis(10));
    let match_request = request(
        &fixture,
        "/internal/cache/v1/match",
        Body::from(cache_request("default", None, &serde_json::json!([]))),
    );
    let match_service = service.clone();
    let matched = tokio::spawn(async move { match_service.handle(match_request).await });
    wait_for_artifact_request(&fixture, &digest, "GET").await;

    assert_eq!(
        service
            .manager
            .purge_worker(fixture.account, fixture.worker, 2_000)
            .unwrap(),
        1
    );
    let gc_store = fixture.artifacts.clone();
    let (acquired_tx, mut acquired_rx) = tokio::sync::oneshot::channel();
    let gc = tokio::spawn(async move {
        let fence = gc_store.fence_deployment_gc().await;
        let _ = acquired_tx.send(());
        gc_store
            .gc_unreferenced(
                &fence,
                &HashSet::new(),
                SystemTime::now() + Duration::from_secs(1),
            )
            .await
    });
    assert!(
        tokio::time::timeout(Duration::from_millis(20), &mut acquired_rx)
            .await
            .is_err(),
        "artifact GC must wait for verified local stream admission"
    );

    let response = tokio::time::timeout(Duration::from_secs(5), matched)
        .await
        .expect("cache match deadline")
        .expect("cache match task");
    assert_eq!(response.status(), StatusCode::OK);
    tokio::time::timeout(Duration::from_secs(1), &mut acquired_rx)
        .await
        .expect("artifact GC fence deadline")
        .expect("artifact GC fence sender");
    assert!(gc.await.expect("artifact GC task").unwrap() >= 1);
    assert_eq!(response_bytes(response).await, body);
}

#[test]
fn cache_header_deadline_and_range_policy_matrix_is_deterministic() {
    assert_eq!(
        canonical_header_map(vec![
            ("X-Test".to_owned(), "one".to_owned()),
            ("x-test".to_owned(), "two".to_owned()),
        ])
        .unwrap()["x-test"],
        "one, two"
    );
    for values in [
        vec![(String::new(), "value".to_owned())],
        vec![("name".to_owned(), "bad\nvalue".to_owned())],
    ] {
        assert_eq!(
            canonical_headers(values).unwrap_err().code(),
            ErrorCode::CacheProtocolError
        );
    }
    assert!(comma_values(&[], "vary").unwrap().is_empty());
    assert_eq!(
        comma_values(
            &[CacheHeader {
                name: "vary".to_owned(),
                value: "Accept, accept, X-Test".to_owned(),
            }],
            "vary",
        )
        .unwrap(),
        vec!["accept", "x-test"]
    );
    assert!(
        comma_values(
            &[CacheHeader {
                name: "vary".to_owned(),
                value: "accept,".to_owned()
            }],
            "vary"
        )
        .is_err()
    );
    for value in ["no-store", "No-Cache=\"field\"", "max-age=1, PRIVATE"] {
        assert!(has_forbidden_cache_directive(value));
    }
    assert!(!has_forbidden_cache_directive("max-age=1, immutable"));

    let automatic = vec![CacheHeader {
        name: "cloudflare-cdn-cache-control".to_owned(),
        value: "max-age=1, s-maxage=2, stale-while-revalidate=3, stale-if-error=4".to_owned(),
    }];
    assert_eq!(
        cache_deadlines(&automatic, 10, CacheSurface::Automatic, 200).unwrap(),
        Some((2_010, 5_010, 6_010))
    );
    for directive in ["no-store", "max-age=invalid"] {
        assert_eq!(
            cache_deadlines(
                &[CacheHeader {
                    name: "cache-control".to_owned(),
                    value: directive.to_owned(),
                }],
                0,
                CacheSurface::Automatic,
                200,
            )
            .unwrap_err()
            .code(),
            ErrorCode::CachePutRejected
        );
    }
    for (status, seconds) in [(203, 7_200), (300, 1_200), (404, 180), (405, 60)] {
        assert_eq!(
            cache_deadlines(&[], 0, CacheSurface::CacheApiDefault, status).unwrap(),
            Some((seconds * 1_000, seconds * 1_000, seconds * 1_000))
        );
    }

    let stored = CacheStoredResponse {
        status: 200,
        headers: Vec::new(),
        body: CacheBodyRef {
            sha256: "11".repeat(32),
            size: 4,
        },
        vary: Vec::new(),
        tags: vec!["tag".to_owned()],
        fresh_until_ms: 10,
        stale_while_revalidate_until_ms: 10,
        stale_if_error_until_ms: 10,
        generation: 1,
    };
    let mut response_headers = HeaderMap::from_iter([
        (header::ETAG, HeaderValue::from_static("\"v1\"")),
        (
            header::LAST_MODIFIED,
            HeaderValue::from_static("Sun, 06 Nov 1994 08:49:37 GMT"),
        ),
    ]);
    assert_eq!(
        cached_response_plan(
            &stored,
            &BTreeMap::from([(
                "if-modified-since".to_owned(),
                "Sun, 06 Nov 1994 08:49:38 GMT".to_owned(),
            )]),
            CacheMethod::Get,
            &mut response_headers,
        )
        .unwrap(),
        CachedResponsePlan::Empty { status: 304 }
    );
    for range in ["items=0-1", "bytes=0-1,3-4", "bytes=bad", "bytes=5-1"] {
        let mut headers = HeaderMap::new();
        assert_eq!(
            cached_response_plan(
                &stored,
                &BTreeMap::from([("range".to_owned(), range.to_owned())]),
                CacheMethod::Get,
                &mut headers,
            )
            .unwrap(),
            CachedResponsePlan::Empty { status: 416 }
        );
    }
    for (range, expected) in [
        (
            "bytes=-2",
            CachedResponsePlan::Range {
                start: 2,
                length: 2,
            },
        ),
        (
            "bytes=2-",
            CachedResponsePlan::Range {
                start: 2,
                length: 2,
            },
        ),
    ] {
        let mut headers = HeaderMap::new();
        assert_eq!(
            cached_response_plan(
                &stored,
                &BTreeMap::from([("range".to_owned(), range.to_owned())]),
                CacheMethod::Get,
                &mut headers,
            )
            .unwrap(),
            expected
        );
    }
    let mut headers = HeaderMap::new();
    assert_eq!(
        cached_response_plan(
            &stored,
            &BTreeMap::from([("range".to_owned(), "bytes=0-1".to_owned())]),
            CacheMethod::Head,
            &mut headers,
        )
        .unwrap(),
        CachedResponsePlan::Full
    );
}
