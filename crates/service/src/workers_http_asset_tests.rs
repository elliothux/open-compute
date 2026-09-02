use super::*;
use crate::asset_backend::{AssetBindingService, serve_asset_plan};
use axum::body::to_bytes;
use open_compute_artifacts::ArtifactCache;
use open_compute_core::{CacheConfig, StartupId, VersionUploadId};
use open_compute_workers::{
    AssetEntryV1, AssetResponsePlan, CanonicalBundle, ModuleInput, ModuleType,
};
use sha2::{Digest, Sha256};
use tower::ServiceExt;

fn authorized(request: axum::http::request::Builder, body: Body) -> Request {
    request
        .header(header::AUTHORIZATION, "Bearer account-admin")
        .body(body)
        .unwrap()
}

async fn response_json(response: Response) -> serde_json::Value {
    serde_json::from_slice(&to_bytes(response.into_body(), 128 * 1024).await.unwrap()).unwrap()
}

async fn create_upload(
    router: &axum::Router,
    collection: &str,
    key: &str,
    body: &serde_json::Value,
) -> serde_json::Value {
    let response = router
        .clone()
        .oneshot(authorized(
            Request::builder()
                .method("POST")
                .uri(collection)
                .header(IDEMPOTENCY_HEADER, key)
                .header(header::CONTENT_TYPE, "application/json"),
            Body::from(serde_json::to_vec(body).unwrap()),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    response_json(response).await
}

#[tokio::test]
async fn asset_upload_endpoints_and_private_binding_fail_closed() {
    let (_temp, _mock, api, account) = tests::worker_api_fixture().await;
    let (worker, _) = WorkerRepository::new(api.storage.db())
        .create_worker(account, "asset-protocol", RequestId::generate(), 1, 100)
        .unwrap();
    let state = tests::authorized_http_state(api.clone());
    let router = control_router().with_state(state);
    let collection = format!(
        "/v1/accounts/{account}/workers/{}/version-uploads",
        worker.id
    );
    let bytes = b"<main>private asset binding</main>";
    let digest = hex::encode(Sha256::digest(bytes));
    let create = serde_json::json!({
        "contentKind": "assets_only",
        "manifest": {
            "schemaVersion": 1,
            "entries": [{
                "path": "/index.html",
                "sha256": digest,
                "size": bytes.len(),
                "contentType": "text/html; charset=utf-8"
            }]
        },
        "routing": {
            "schemaVersion": 1,
            "runWorkerFirst": false,
            "htmlHandling": "auto-trailing-slash",
            "notFoundHandling": "404-page",
            "headers": [],
            "redirects": []
        }
    });

    for (method, uri) in [
        ("POST", collection.clone()),
        ("GET", format!("{collection}/bad")),
        ("PUT", format!("{collection}/bad/objects/{digest}")),
        ("POST", format!("{collection}/bad/finalize")),
        ("DELETE", format!("{collection}/bad")),
    ] {
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(uri)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    let invalid_requests = [
        authorized(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/v1/accounts/bad/workers/{}/version-uploads",
                    worker.id
                ))
                .header(IDEMPOTENCY_HEADER, "bad-account"),
            Body::from("{}"),
        ),
        authorized(
            Request::builder()
                .method("POST")
                .uri(&collection)
                .header(header::CONTENT_TYPE, "application/json"),
            Body::from("{}"),
        ),
        authorized(
            Request::builder()
                .method("POST")
                .uri(&collection)
                .header(IDEMPOTENCY_HEADER, "bad-json")
                .header(header::CONTENT_TYPE, "application/json"),
            Body::from("{"),
        ),
        authorized(
            Request::builder()
                .method("GET")
                .uri(format!("{collection}/bad")),
            Body::empty(),
        ),
        authorized(
            Request::builder()
                .method("PUT")
                .uri(format!("{collection}/bad/objects/{digest}")),
            Body::empty(),
        ),
        authorized(
            Request::builder().method("PUT").uri(format!(
                "{collection}/{}/objects/bad",
                VersionUploadId::generate()
            )),
            Body::empty(),
        ),
        authorized(
            Request::builder()
                .method("POST")
                .uri(format!("{collection}/bad/finalize")),
            Body::empty(),
        ),
        authorized(
            Request::builder().method("POST").uri(format!(
                "{collection}/{}/finalize",
                VersionUploadId::generate()
            )),
            Body::from("{"),
        ),
        authorized(
            Request::builder()
                .method("DELETE")
                .uri(format!("{collection}/bad")),
            Body::empty(),
        ),
    ];
    for request in invalid_requests {
        let response = router.clone().oneshot(request).await.unwrap();
        assert!(response.status().is_client_error());
    }

    let mismatch = serde_json::json!({
        "contentKind": "worker",
        "manifest": create["manifest"].clone(),
        "routing": create["routing"].clone()
    });
    let response = router
        .clone()
        .oneshot(authorized(
            Request::builder()
                .method("POST")
                .uri(&collection)
                .header(IDEMPOTENCY_HEADER, "missing-bundle")
                .header(header::CONTENT_TYPE, "application/json"),
            Body::from(serde_json::to_vec(&mismatch).unwrap()),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);

    let unknown_worker_collection = format!(
        "/v1/accounts/{account}/workers/{}/version-uploads",
        WorkerId::generate()
    );
    let response = router
        .clone()
        .oneshot(authorized(
            Request::builder()
                .method("POST")
                .uri(unknown_worker_collection)
                .header(IDEMPOTENCY_HEADER, "unknown-worker")
                .header(header::CONTENT_TYPE, "application/json"),
            Body::from(serde_json::to_vec(&create).unwrap()),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let created = create_upload(&router, &collection, "asset-private", &create).await;
    let upload = created["id"].as_str().unwrap();
    let item = format!("{collection}/{upload}");
    let response = router
        .clone()
        .oneshot(authorized(
            Request::builder().method("GET").uri(&item),
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response_json(response).await["status"], "open");

    let response = router
        .clone()
        .oneshot(authorized(
            Request::builder()
                .method("PUT")
                .uri(format!("{item}/objects/{}", hex::encode([9; 32]))),
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let response = router
        .clone()
        .oneshot(authorized(
            Request::builder()
                .method("PUT")
                .uri(format!("{item}/objects/{digest}"))
                .header(header::CONTENT_LENGTH, bytes.len() + 1),
            Body::from(bytes.as_slice()),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let response = router
        .clone()
        .oneshot(authorized(
            Request::builder()
                .method("PUT")
                .uri(format!("{item}/objects/{digest}")),
            Body::from(bytes.as_slice()),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let metadata = serde_json::json!({
        "promote": true
    });
    let response = router
        .clone()
        .oneshot(authorized(
            Request::builder()
                .method("POST")
                .uri(format!("{item}/finalize"))
                .header(header::CONTENT_TYPE, "application/json"),
            Body::from(serde_json::to_vec(&metadata).unwrap()),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    let finalized = response_json(response).await;
    let version_id: VersionId = finalized["version"]["id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let version = WorkerRepository::new(api.storage.db())
        .get_version(account, worker.id, version_id)
        .unwrap();

    let Err(error) = serve_asset_plan(
        &api.storage,
        &api.artifacts,
        None,
        &version,
        AssetResponsePlan {
            status: 200,
            entry: Some(AssetEntryV1 {
                path: "/index.html".to_owned(),
                sha256: digest.clone(),
                size: bytes.len() as u64 + 1,
                content_type: "text/html".to_owned(),
            }),
            headers: BTreeMap::new(),
            head: false,
        },
    )
    .await
    else {
        panic!("mismatched asset authority unexpectedly served bytes");
    };
    assert_eq!(error.code(), ErrorCode::VersionNotFound);

    let cache = Arc::new(
        ArtifactCache::open(
            api.storage.data_dir().artifact_cache_dir(),
            CacheConfig::default(),
            StartupId::generate(),
        )
        .unwrap(),
    );
    let binding = AssetBindingService::new(
        api.storage.clone(),
        api.artifacts.clone(),
        cache,
        api.pins.clone(),
    );
    let descriptor = hex::encode(version.worker_code_sha256);
    let binding_request = |method: &str, url: &str, descriptor: &str| {
        Request::builder()
            .header("x-open-compute-version-id", version_id.to_string())
            .header("x-open-compute-descriptor-sha256", descriptor)
            .header("x-open-compute-asset-method", method)
            .header("x-open-compute-asset-url", url)
            .body(Body::empty())
            .unwrap()
    };

    let response = binding.handle(Request::new(Body::empty())).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        response.headers()["x-open-compute-error-code"],
        ErrorCode::BindingProtocolError.as_str()
    );
    let response = binding
        .handle(binding_request("GET", "ftp://example.test/", &descriptor))
        .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let response = binding
        .handle(binding_request(
            "GET",
            "https://example.test/",
            &"0".repeat(64),
        ))
        .await;
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(
        response.headers()["x-open-compute-error-code"],
        ErrorCode::AssetIntegrityError.as_str()
    );

    let mut request = binding_request("GET", "https://example.test/?source=binding", &descriptor);
    request
        .headers_mut()
        .insert("sec-fetch-mode", HeaderValue::from_static("navigate"));
    request
        .headers_mut()
        .insert(header::AUTHORIZATION, HeaderValue::from_static("tenant"));
    let response = binding.handle(request).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(api.pins.count(version_id), 1);
    assert_eq!(
        to_bytes(response.into_body(), 64 * 1024).await.unwrap(),
        bytes.as_slice()
    );
    assert_eq!(api.pins.count(version_id), 0);

    let response = binding
        .handle(binding_request(
            "HEAD",
            "https://example.test/",
            &descriptor,
        ))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()["x-open-compute-asset-representation-length"],
        bytes.len().to_string()
    );
    assert!(response.headers().get(header::CONTENT_LENGTH).is_none());
    assert!(
        to_bytes(response.into_body(), 1024)
            .await
            .unwrap()
            .is_empty()
    );

    let aborted = create_upload(&router, &collection, "asset-abort", &create).await;
    let response = router
        .clone()
        .oneshot(authorized(
            Request::builder()
                .method("DELETE")
                .uri(format!("{collection}/{}", aborted["id"].as_str().unwrap())),
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response_json(response).await["status"], "aborted");
}

#[tokio::test]
async fn worker_asset_upload_stages_bundle_and_replays_runtime_failure() {
    let (_temp, _mock, api, account) = tests::worker_api_fixture().await;
    let (worker, _) = WorkerRepository::new(api.storage.db())
        .create_worker(account, "asset-worker", RequestId::generate(), 1, 100)
        .unwrap();
    let router = control_router().with_state(tests::authorized_http_state(api));
    let collection = format!(
        "/v1/accounts/{account}/workers/{}/version-uploads",
        worker.id
    );
    let bundle = CanonicalBundle::build(
        "index.js",
        vec![ModuleInput {
            name: "index.js".to_owned(),
            module_type: ModuleType::EsModule,
            bytes: b"export default { fetch() { return new Response('ok'); } };".to_vec(),
        }],
        BundleLimits::default(),
    )
    .unwrap()
    .into_bytes();
    let bundle_digest = hex::encode(Sha256::digest(&bundle));
    let asset = b"worker asset";
    let asset_digest = hex::encode(Sha256::digest(asset));
    let create = serde_json::json!({
        "contentKind": "worker",
        "bundle": { "sha256": bundle_digest, "size": bundle.len() },
        "manifest": {
            "schemaVersion": 1,
            "entries": [{
                "path": "/asset.txt",
                "sha256": asset_digest,
                "size": asset.len(),
                "contentType": "text/plain"
            }]
        },
        "routing": {
            "schemaVersion": 1,
            "runWorkerFirst": true,
            "htmlHandling": "none",
            "notFoundHandling": "none",
            "headers": [],
            "redirects": []
        }
    });
    let created = create_upload(&router, &collection, "worker-assets", &create).await;
    let upload = created["id"].as_str().unwrap();
    let response = router
        .clone()
        .oneshot(authorized(
            Request::builder()
                .method("PUT")
                .uri(format!("{collection}/{upload}/objects/{bundle_digest}")),
            Body::from(bundle.clone()),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let response = router
        .clone()
        .oneshot(authorized(
            Request::builder()
                .method("PUT")
                .uri(format!("{collection}/{upload}/objects/{asset_digest}")),
            Body::from(asset.as_slice()),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let metadata = serde_json::json!({
        "mainModule": "index.js"
    });
    for _ in 0..2 {
        let response = router
            .clone()
            .oneshot(authorized(
                Request::builder()
                    .method("POST")
                    .uri(format!("{collection}/{upload}/finalize"))
                    .header(header::CONTENT_TYPE, "application/json"),
                Body::from(serde_json::to_vec(&metadata).unwrap()),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(
            response_json(response).await["error"]["code"],
            ErrorCode::RuntimeUnavailable.as_str()
        );
    }

    let mismatch = create_upload(&router, &collection, "worker-assets-mismatch", &create).await;
    let mismatch_upload = mismatch["id"].as_str().unwrap();
    for (digest, body) in [
        (bundle_digest.as_str(), Body::from(bundle)),
        (asset_digest.as_str(), Body::from(asset.as_slice())),
    ] {
        let response = router
            .clone()
            .oneshot(authorized(
                Request::builder()
                    .method("PUT")
                    .uri(format!("{collection}/{mismatch_upload}/objects/{digest}")),
                body,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }
    let wrong_metadata = serde_json::json!({
        "mainModule": "wrong.js"
    });
    for _ in 0..2 {
        let response = router
            .clone()
            .oneshot(authorized(
                Request::builder()
                    .method("POST")
                    .uri(format!("{collection}/{mismatch_upload}/finalize"))
                    .header(header::CONTENT_TYPE, "application/json"),
                Body::from(serde_json::to_vec(&wrong_metadata).unwrap()),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            response_json(response).await["error"]["code"],
            ErrorCode::BundleInvalid.as_str()
        );
    }
}
