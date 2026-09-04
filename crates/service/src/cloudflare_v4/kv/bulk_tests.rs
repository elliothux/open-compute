use axum::body::{Body, to_bytes};
use axum::http::{Method, Request, StatusCode, header};
use open_compute_artifacts::{
    ArtifactStore, MapEnv, S3ArtifactClient, resolve_s3_credentials_with,
};
use open_compute_core::{
    BindingKind, KvConfig, PlatformConfig, PlatformId, RequestId, SecretString, SystemClock,
};
use open_compute_workers::{
    CreateResourceOutcome, CreateResourceRequest, KvResourceDriver, ResourceController,
    ResourcePins,
};
use std::sync::Arc;
use std::time::Duration;
use tower::ServiceExt as _;

fn artifact_store(mock: &open_compute_artifacts::MockS3) -> ArtifactStore {
    let config = PlatformConfig::from_toml_str(&format!(
        r#"
[s3]
endpoint = "{}"
bucket = "open-compute"
prefix = "system/"
connect_timeout_ms = 100
request_timeout_ms = 1000
"#,
        mock.endpoint
    ))
    .unwrap()
    .s3;
    let env = MapEnv::new()
        .with("S3_ACCESS_KEY_ID", "test-access")
        .with("S3_SECRET_ACCESS_KEY", "test-secret");
    let credentials = resolve_s3_credentials_with(&config, &env).unwrap();
    ArtifactStore::new(S3ArtifactClient::connect(&config, &credentials, 1024 * 1024).unwrap())
}

async fn json(response: axum::response::Response) -> serde_json::Value {
    serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await.unwrap()).unwrap()
}

#[tokio::test]
async fn bulk_routes_cover_text_json_metadata_and_validation() {
    let (_temp, mock, state, account, storage) =
        crate::tests::initialized_worker_http_fixture().await;
    let pins = ResourcePins::new();
    let created = ResourceController::new(
        &storage,
        pins.clone(),
        KvResourceDriver::new(&storage, 256 * 1024 * 1024),
    )
    .create(&CreateResourceRequest {
        account_id: account,
        kind: BindingKind::KvNamespace,
        name: "bulk-namespace".to_owned(),
        idempotency_key: "bulk-create".to_owned(),
        driver_schema_version: 1,
        request_id: RequestId::generate(),
        now_ms: 1,
    })
    .unwrap();
    let resource_id = match created {
        CreateResourceOutcome::Applied(value) => value.resource_id,
        CreateResourceOutcome::Replay(_) => unreachable!(),
    };
    let metrics = state.metrics().clone();
    let executor = Arc::new(
        crate::kv_backend::SqliteKvBindingExecutor::new(storage.clone(), Arc::new(SystemClock))
            .with_metrics(metrics),
    );
    let api = crate::kv_api::KvApiState::new(
        storage.clone(),
        artifact_store(&mock),
        pins,
        executor,
        KvConfig::default(),
        100,
        Duration::from_millis(10),
    );
    let authority =
        super::super::super::accounts::AccountAuthority::new(PlatformId::generate(), account, 1);
    let public_account = authority.public_id().to_owned();
    let namespace = authority.public_resource_id(
        super::super::super::V4ResourceKind::KvNamespace,
        resource_id,
    );
    let app = crate::http::admin_router(
        state
            .with_kv_api(api)
            .with_platform_storage(storage.clone())
            .with_v4_tokens(
                SecretString::new("deployer-token"),
                SecretString::new("read-token"),
            )
            .with_cloudflare_v4_account(authority),
    );
    let prefix =
        format!("/client/v4/accounts/{public_account}/storage/kv/namespaces/{namespace}/bulk");
    let call = |method: Method, suffix: &str, value: serde_json::Value| {
        Request::builder()
            .method(method)
            .uri(format!("{prefix}{suffix}"))
            .header(header::AUTHORIZATION, "Bearer deployer-token")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(serde_json::to_vec(&value).unwrap()))
            .unwrap()
    };

    let updated = app
        .clone()
        .oneshot(call(
            Method::PUT,
            "",
            serde_json::json!([
                {"key":"text","value":"hello","metadata":{"kind":"text"}},
                {"key":"json","value":"eyJvayI6dHJ1ZX0=","base64":true,"expiration_ttl":60}
            ]),
        ))
        .await
        .unwrap();
    assert_eq!(updated.status(), StatusCode::OK);
    assert_eq!(json(updated).await["result"]["successful_key_count"], 2);

    let text = app
        .clone()
        .oneshot(call(
            Method::POST,
            "/get",
            serde_json::json!({"keys":["text","missing"],"type":"text","withMetadata":true}),
        ))
        .await
        .unwrap();
    assert_eq!(text.status(), StatusCode::OK);
    let text = json(text).await;
    assert_eq!(text["result"]["values"]["text"]["value"], "hello");
    assert_eq!(text["result"]["values"]["missing"], serde_json::Value::Null);

    let json_value = app
        .clone()
        .oneshot(call(
            Method::POST,
            "/get",
            serde_json::json!({"keys":["json"],"type":"json"}),
        ))
        .await
        .unwrap();
    assert_eq!(json_value.status(), StatusCode::OK);
    assert_eq!(
        json(json_value).await["result"]["values"]["json"]["ok"],
        true
    );

    for value in [
        serde_json::json!([{"key":"bad","value":"%%%","base64":true}]),
        serde_json::json!([{"key":"bad","value":"x","expiration_ttl":59}]),
        serde_json::json!([{"key":"","value":"x"}]),
    ] {
        let response = app
            .clone()
            .oneshot(call(Method::PUT, "", value))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
    let invalid_json_value = app
        .clone()
        .oneshot(call(
            Method::POST,
            "/get",
            serde_json::json!({"keys":["text"],"type":"json"}),
        ))
        .await
        .unwrap();
    assert_eq!(invalid_json_value.status(), StatusCode::BAD_REQUEST);
    let invalid_key = app
        .clone()
        .oneshot(call(Method::POST, "/get", serde_json::json!({"keys":[""]})))
        .await
        .unwrap();
    assert_eq!(invalid_key.status(), StatusCode::BAD_REQUEST);

    let namespace_prefix = prefix.strip_suffix("/bulk").unwrap();
    let multipart = concat!(
        "--open-compute\r\n",
        "Content-Disposition: form-data; name=\"value\"\r\n\r\n",
        "multipart-value\r\n",
        "--open-compute\r\n",
        "Content-Disposition: form-data; name=\"metadata[nested][key]\"\r\n\r\n",
        "metadata-value\r\n",
        "--open-compute--\r\n"
    );
    let multipart_put = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri(format!(
                    "{namespace_prefix}/values/multipart?expiration_ttl=60"
                ))
                .header(header::AUTHORIZATION, "Bearer deployer-token")
                .header(
                    header::CONTENT_TYPE,
                    "multipart/form-data; boundary=open-compute",
                )
                .body(Body::from(multipart))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(multipart_put.status(), StatusCode::OK);

    let raw = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("{namespace_prefix}/values/multipart"))
                .header(header::AUTHORIZATION, "Bearer read-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(raw.status(), StatusCode::OK);
    assert_eq!(
        raw.headers()[header::CONTENT_TYPE],
        "application/octet-stream"
    );
    assert!(raw.headers().contains_key("expiration"));
    assert_eq!(
        to_bytes(raw.into_body(), 1024).await.unwrap(),
        "multipart-value"
    );

    let metadata = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("{namespace_prefix}/metadata/multipart"))
                .header(header::AUTHORIZATION, "Bearer read-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(metadata.status(), StatusCode::OK);
    assert_eq!(
        json(metadata).await["result"],
        serde_json::json!({"nested":{"key":"metadata-value"}})
    );

    for uri in [
        format!("{namespace_prefix}/values/multipart?expiration_ttl=59"),
        format!("{namespace_prefix}/values/multipart?unexpected=true"),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::PUT)
                    .uri(uri)
                    .header(header::AUTHORIZATION, "Bearer deployer-token")
                    .body(Body::from("value"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    let missing = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("{namespace_prefix}/values/missing"))
                .header(header::AUTHORIZATION, "Bearer read-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);

    let raw_delete = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::DELETE)
                .uri(format!("{namespace_prefix}/values/multipart"))
                .header(header::AUTHORIZATION, "Bearer deployer-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(raw_delete.status(), StatusCode::OK);

    let deleted = app
        .clone()
        .oneshot(call(
            Method::POST,
            "/delete",
            serde_json::json!(["text", "json"]),
        ))
        .await
        .unwrap();
    assert_eq!(deleted.status(), StatusCode::OK);
    assert_eq!(json(deleted).await["result"]["successful_key_count"], 2);

    let query = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("{prefix}/delete?unexpected=true"))
                .header(header::AUTHORIZATION, "Bearer deployer-token")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from("[]"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(query.status(), StatusCode::BAD_REQUEST);

    let catalog_prefix = format!("/client/v4/accounts/{public_account}/storage/kv/namespaces");
    let listed = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!(
                    "{catalog_prefix}?page=1&per_page=10&order=title&direction=asc"
                ))
                .header(header::AUTHORIZATION, "Bearer read-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(listed.status(), StatusCode::OK);
    assert_eq!(json(listed).await["result"][0]["title"], "bulk-namespace");

    let renamed = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri(format!("{catalog_prefix}/{namespace}"))
                .header(header::AUTHORIZATION, "Bearer deployer-token")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"title":"renamed-namespace"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(renamed.status(), StatusCode::OK);
    assert_eq!(json(renamed).await["result"]["title"], "renamed-namespace");

    let fetched = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("{catalog_prefix}/{namespace}"))
                .header(header::AUTHORIZATION, "Bearer read-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(fetched.status(), StatusCode::OK);

    let invalid_catalog = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("{catalog_prefix}?page=0"))
                .header(header::AUTHORIZATION, "Bearer read-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(invalid_catalog.status(), StatusCode::BAD_REQUEST);

    let backups = format!(
        "/client/v4/accounts/{public_account}/open-compute/kv/namespaces/{namespace}/backups"
    );
    let backup = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(&backups)
                .header(header::AUTHORIZATION, "Bearer admin-token")
                .header("idempotency-key", "kv-backup")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(backup.status(), StatusCode::OK);
    let backup_id = json(backup).await["result"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let listed_backups = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(&backups)
                .header(header::AUTHORIZATION, "Bearer read-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(listed_backups.status(), StatusCode::OK);
    assert_eq!(
        json(listed_backups).await["result"]
            .as_array()
            .unwrap()
            .len(),
        1
    );

    let restored = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!(
                    "/client/v4/accounts/{public_account}/open-compute/kv/backups/{backup_id}/restore"
                ))
                .header(header::AUTHORIZATION, "Bearer admin-token")
                .header(header::CONTENT_TYPE, "application/json")
                .header("idempotency-key", "kv-restore")
                .body(Body::from(r#"{"name":"restored-kv"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(restored.status(), StatusCode::OK);
    assert_eq!(json(restored).await["result"]["name"], "restored-kv");

    for key in [String::new(), "has whitespace".to_owned(), "x".repeat(129)] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(&backups)
                    .header(header::AUTHORIZATION, "Bearer admin-token")
                    .header("idempotency-key", key)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
}
