use super::*;
use axum::body::{Body, to_bytes};
use axum::http::{Method, Request, StatusCode, header};
use open_compute_core::{AccountId, PlatformId, SecretString};
use tower::ServiceExt as _;

async fn json(response: Response) -> serde_json::Value {
    serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await.unwrap()).unwrap()
}

#[test]
fn hashes_routing_tokens_and_buckets_cover_supported_shapes() {
    let html = wrangler_hash(b"good", "/index.html");
    assert_eq!(html, "4c73266e449fea54bba5a6dea074dbbd");
    assert_ne!(html, wrangler_hash(b"bad!", "/index.html"));
    assert_ne!(html, wrangler_hash(b"good", "/index.txt"));
    assert_eq!(
        wrangler_hash(b"good", "/.well-known"),
        wrangler_hash(b"good", "/extensionless")
    );

    for (html, not_found) in [
        (None, None),
        (Some("auto-trailing-slash"), Some("none")),
        (Some("force-trailing-slash"), Some("404-page")),
        (Some("drop-trailing-slash"), Some("single-page-application")),
        (Some("none"), Some("none")),
    ] {
        let routing = routing_config(
            Some("ASSETS".to_owned()),
            &WorkerUploadAssetsConfig {
                html_handling: html.map(str::to_owned),
                not_found_handling: not_found.map(str::to_owned),
                run_worker_first: Some(serde_json::json!(["/api/*", "!/api/assets/*"])),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(routing.binding.as_deref(), Some("ASSETS"));
    }
    assert!(
        routing_config(
            None,
            &WorkerUploadAssetsConfig {
                run_worker_first: Some(serde_json::json!(true)),
                ..Default::default()
            }
        )
        .is_ok()
    );
    for config in [
        WorkerUploadAssetsConfig {
            html_handling: Some("invalid".to_owned()),
            ..Default::default()
        },
        WorkerUploadAssetsConfig {
            not_found_handling: Some("invalid".to_owned()),
            ..Default::default()
        },
        WorkerUploadAssetsConfig {
            run_worker_first: Some(serde_json::json!([1])),
            ..Default::default()
        },
        WorkerUploadAssetsConfig {
            run_worker_first: Some(serde_json::json!({})),
            ..Default::default()
        },
        WorkerUploadAssetsConfig {
            _headers: Some("/path".to_owned()),
            ..Default::default()
        },
        WorkerUploadAssetsConfig {
            _redirects: Some("/old /new".to_owned()),
            ..Default::default()
        },
    ] {
        assert!(routing_config(None, &config).is_err());
    }
}

#[tokio::test]
async fn asset_session_single_upload_and_completion_token_round_trip() {
    let (_temp, _mock, state, account, _storage) =
        crate::tests::initialized_worker_http_fixture().await;
    let authority =
        crate::cloudflare_v4::accounts::AccountAuthority::new(PlatformId::generate(), account, 1);
    let public_account = authority.public_id().to_owned();
    let configured = state
        .with_v4_tokens(
            SecretString::new("deployer-token"),
            SecretString::new("read-token"),
        )
        .with_cloudflare_v4_account(authority);
    let api = configured.worker_api().unwrap().clone();
    let app = crate::http::admin_router(configured.clone());
    let content = b"asset body";
    let hash = wrangler_hash(content, "/index.html");
    let create_uri =
        format!("/client/v4/accounts/{public_account}/workers/scripts/site/assets-upload-session");
    let created = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(&create_uri)
                .header(header::AUTHORIZATION, "Bearer deployer-token")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_vec(&serde_json::json!({
                        "manifest":{"/index.html":{"hash":hash,"size":content.len()}}
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::OK);
    let created = json(created).await;
    assert_eq!(created["result"]["buckets"][0][0], hash);
    let upload_token = created["result"]["jwt"].as_str().unwrap();
    assert!(authenticate_upload_token(
        &configured,
        &format!("/client/v4/accounts/{public_account}/workers/assets/upload/{hash}"),
        Some(upload_token)
    ));
    assert!(!authenticate_upload_token(
        &configured,
        "/wrong/path",
        Some(upload_token)
    ));
    assert!(!authenticate_upload_token(
        &configured,
        &format!("/client/v4/accounts/{public_account}/workers/assets/upload/{hash}"),
        None
    ));
    for token in ["", "a.b", "a.b.c.d", "a.%%%.c", "a.YQ.c"] {
        assert!(open_token(&api, token).is_err());
    }

    let upload_uri = format!("/client/v4/accounts/{public_account}/workers/assets/upload/{hash}");
    let conflict = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(&upload_uri)
                .header(header::AUTHORIZATION, format!("Bearer {upload_token}"))
                .header(header::CONTENT_TYPE, "text/html")
                .body(Body::from("wrong"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(conflict.status(), StatusCode::CONFLICT);

    let uploaded = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(&upload_uri)
                .header(header::AUTHORIZATION, format!("Bearer {upload_token}"))
                .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
                .body(Body::from(content.as_slice()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(uploaded.status(), StatusCode::CREATED);
    let complete_token = json(uploaded).await["result"]["jwt"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(open_token(&api, &complete_token).is_ok());
    assert!(!authenticate_upload_token(
        &configured,
        &upload_uri,
        Some(&complete_token)
    ));

    let (assets, reservation) = redeem_assets(
        &api,
        &complete_token,
        account,
        "site",
        None,
        Some("ASSETS".to_owned()),
        &WorkerUploadAssetsConfig::default(),
        0,
    )
    .unwrap();
    assert_eq!(assets.manifest.entries.len(), 1);
    assert_eq!(assets.routing.binding.as_deref(), Some("ASSETS"));
    assert!(reservation.operation_id.is_none());
    assert!(consume_assets(&api, &reservation, 1).is_err());
    assert!(release_assets(&api, &reservation, 1).is_err());
    assert!(
        redeem_assets(
            &api,
            &complete_token,
            AccountId::generate(),
            "site",
            None,
            None,
            &WorkerUploadAssetsConfig::default(),
            0,
        )
        .is_err()
    );

    for (uri, body) in [
        (format!("{create_uri}?invalid=true"), serde_json::json!({})),
        (
            create_uri,
            serde_json::json!({
                "manifest":{
                    "/same":{"hash":"one","size":1},
                    "same":{"hash":"two","size":1}
                }
            }),
        ),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(uri)
                    .header(header::AUTHORIZATION, "Bearer deployer-token")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
}

#[tokio::test]
async fn bulk_upload_accepts_the_exact_base64_multipart_contract() {
    let (temp, _mock, state, account, _storage) =
        crate::tests::initialized_worker_http_fixture().await;
    let mut loaded =
        crate::config_load::load_platform_config(&temp.path().join("config.toml")).unwrap();
    let s3 = loaded.config.object_storage.as_s3_mut().unwrap();
    s3.normalize_implicit_env_defaults();
    let credentials = open_compute_artifacts::resolve_s3_credentials(s3).unwrap();
    let mut worker_api = state.worker_api().unwrap().as_ref().clone();
    worker_api.artifacts = open_compute_artifacts::ArtifactStore::new(
        open_compute_artifacts::ObjectBackend::connect_s3(s3, &credentials, 25 * 1024 * 1024)
            .unwrap(),
    );
    let state = state.with_worker_api(worker_api);
    let authority =
        crate::cloudflare_v4::accounts::AccountAuthority::new(PlatformId::generate(), account, 1);
    let public_account = authority.public_id().to_owned();
    let configured = state
        .with_v4_tokens(
            SecretString::new("deployer-token"),
            SecretString::new("read-token"),
        )
        .with_cloudflare_v4_account(authority);
    let api = configured.worker_api().unwrap().clone();
    let app = crate::http::admin_router(configured);
    // One ordinary binary asset exceeds Axum's implicit 2 MiB multipart cap.
    let first = &vec![0xff; 2 * 1024 * 1024 + 1];
    let second = b"second asset";
    let first_hash = wrangler_hash(first, "/first.txt");
    let second_hash = wrangler_hash(second, "/second.txt");
    let create_uri = format!(
        "/client/v4/accounts/{public_account}/workers/scripts/bulk-site/assets-upload-session"
    );
    let created = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(create_uri)
                .header(header::AUTHORIZATION, "Bearer deployer-token")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_vec(&serde_json::json!({
                        "manifest":{
                            "/first.txt":{"hash":first_hash,"size":first.len()},
                            "/second.txt":{"hash":second_hash,"size":second.len()}
                        }
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::OK);
    let upload_token = json(created).await["result"]["jwt"]
        .as_str()
        .unwrap()
        .to_owned();

    let boundary = "asset-bulk-boundary";
    // Keep the product's encoded-byte budget even without Content-Length.
    let oversized = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"{first_hash}\"\r\n\r\n{}\r\n--{boundary}--\r\n",
        "A".repeat(MAX_UPLOAD_BYTES + 1)
    );
    let rejected = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!(
                    "/client/v4/accounts/{public_account}/workers/assets/upload?base64=true"
                ))
                .header(header::AUTHORIZATION, format!("Bearer {upload_token}"))
                .header(
                    header::CONTENT_TYPE,
                    format!("multipart/form-data; boundary={boundary}"),
                )
                .body(Body::from_stream(stream::iter([Ok::<
                    _,
                    std::convert::Infallible,
                >(
                    Bytes::from(oversized)
                )])))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(rejected.status(), StatusCode::PAYLOAD_TOO_LARGE);
    let encoded_first = base64::engine::general_purpose::STANDARD.encode(first);
    let encoded_second = base64::engine::general_purpose::STANDARD.encode(second);
    let multipart = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"{first_hash}\"\r\nContent-Type: text/plain\r\n\r\n{encoded_first}\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"{second_hash}\"\r\nContent-Type: text/plain\r\n\r\n{encoded_second}\r\n--{boundary}--\r\n"
    );
    let uploaded = app
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!(
                    "/client/v4/accounts/{public_account}/workers/assets/upload?base64=true"
                ))
                .header(header::AUTHORIZATION, format!("Bearer {upload_token}"))
                .header(
                    header::CONTENT_TYPE,
                    format!("multipart/form-data; boundary={boundary}"),
                )
                .body(Body::from(multipart))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(uploaded.status(), StatusCode::CREATED);
    let completed = json(uploaded).await;
    let claims = open_token(&api, completed["result"]["jwt"].as_str().unwrap()).unwrap();
    let session = current_session(&api, &claims).unwrap();
    assert!(
        session
            .entries
            .iter()
            .all(|entry| entry.artifact_sha256.is_some())
    );
    assert_eq!(
        session.entries[0].artifact_sha256,
        Some(Sha256::digest(first).into())
    );
    assert_eq!(
        session.entries[1].artifact_sha256,
        Some(Sha256::digest(second).into())
    );
    for (entry, expected) in session
        .entries
        .iter()
        .zip([first.as_slice(), second.as_slice()])
    {
        let artifact = open_compute_artifacts::ArtifactRef::new(
            open_compute_artifacts::ARTIFACT_KEY_VERSION,
            &hex::encode(entry.artifact_sha256.unwrap()),
            entry.size,
        )
        .unwrap();
        let mut downloaded = Vec::new();
        api.artifacts
            .download_verified(&artifact, &mut downloaded)
            .await
            .unwrap();
        assert_eq!(downloaded, expected);
    }
}
