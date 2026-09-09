use super::*;

#[tokio::test]
async fn v4_asset_upload_auth_integrity_and_failed_script_creation_are_closed() {
    let (_dir, mock, state, account, _storage) = initialized_worker_http_fixture().await;
    let authority = crate::cloudflare_v4::accounts::AccountAuthority::new(
        open_compute_core::PlatformId::generate(),
        account,
        1_000,
    );
    let public_account = authority.public_id().to_owned();
    let state = state
        .with_v4_tokens(
            SecretString::new("deployer-token"),
            SecretString::new("read-token"),
        )
        .with_cloudflare_v4_account(authority);
    let app = http::admin_router(state);
    let missing = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/client/v4/accounts/{public_account}/workers/scripts/missing-worker"
                ))
                .header("authorization", "Bearer read-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    let missing_body = axum::body::to_bytes(missing.into_body(), 64 * 1024)
        .await
        .unwrap();
    let missing_json: serde_json::Value = serde_json::from_slice(&missing_body).unwrap();
    assert_eq!(missing_json["errors"][0]["code"], 10_007);

    let upload_path = format!(
        "/client/v4/accounts/{public_account}/workers/assets/upload/4c73266e449fea54bba5a6dea074dbbd"
    );
    let fake = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(&upload_path)
                .header("authorization", "Bearer eyJ.fake.signature")
                .body(Body::from("good"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(fake.status(), StatusCode::UNAUTHORIZED);

    let session_path = format!(
        "/client/v4/accounts/{public_account}/workers/scripts/assets-worker/assets-upload-session"
    );
    let invalid = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(&session_path)
                .header("authorization", "Bearer admin-token")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"manifest":{"../escape":{"hash":"4c73266e449fea54bba5a6dea074dbbd","size":4}}}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(!invalid.status().is_success());

    let created = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(&session_path)
                .header("authorization", "Bearer admin-token")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"manifest":{"/index.html":{"hash":"4c73266e449fea54bba5a6dea074dbbd","size":4}}}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::OK);
    let created_body = axum::body::to_bytes(created.into_body(), 64 * 1024)
        .await
        .unwrap();
    let created_json: serde_json::Value = serde_json::from_slice(&created_body).unwrap();
    let upload_token = created_json["result"]["jwt"].as_str().unwrap().to_owned();

    let wrong_account = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/client/v4/accounts/00000000000000000000000000000000/workers/assets/upload/4c73266e449fea54bba5a6dea074dbbd")
                .header("authorization", format!("Bearer {upload_token}"))
                .body(Body::from("good"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(wrong_account.status(), StatusCode::UNAUTHORIZED);

    let wrong_content = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(&upload_path)
                .header("authorization", format!("Bearer {upload_token}"))
                .header("content-type", "text/html")
                .body(Body::from("evil"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(!wrong_content.status().is_success());
    assert_eq!(mock.object_count(), 1);

    let uploaded = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(&upload_path)
                .header("authorization", format!("Bearer {upload_token}"))
                .header("content-type", "text/html")
                .body(Body::from("good"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(uploaded.status(), StatusCode::CREATED);
    let uploaded_body = axum::body::to_bytes(uploaded.into_body(), 64 * 1024)
        .await
        .unwrap();
    let uploaded_json: serde_json::Value = serde_json::from_slice(&uploaded_body).unwrap();
    let completion_token = uploaded_json["result"]["jwt"].as_str().unwrap().to_owned();
    let bucket_retry = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(&upload_path)
                .header("authorization", format!("Bearer {upload_token}"))
                .header("content-type", "text/html")
                .body(Body::from("good"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(bucket_retry.status(), StatusCode::CREATED);
    let bucket_retry_body = axum::body::to_bytes(bucket_retry.into_body(), 64 * 1024)
        .await
        .unwrap();
    let bucket_retry_json: serde_json::Value = serde_json::from_slice(&bucket_retry_body).unwrap();
    assert_eq!(bucket_retry_json["result"]["jwt"], completion_token);

    let wrong_purpose = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(&upload_path)
                .header("authorization", format!("Bearer {completion_token}"))
                .body(Body::from("good"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(wrong_purpose.status(), StatusCode::UNAUTHORIZED);

    let metadata = serde_json::json!({
        "main_module": "index.js",
        "compatibility_date": "2026-09-08",
        "assets": {"jwt": completion_token, "config": {}}
    });
    let body = format!(
        "--fixed\r\nContent-Disposition: form-data; name=\"metadata\"\r\nContent-Type: application/json\r\n\r\n{metadata}\r\n--fixed\r\nContent-Disposition: form-data; name=\"index.js\"; filename=\"index.js\"\r\nContent-Type: application/javascript+module\r\n\r\nexport default {{ fetch() {{ return new Response('ok'); }} }};\r\n--fixed--\r\n"
    );
    let upload_script = |name: &str, exclude_script: bool| {
        let query = if exclude_script {
            "excludeScript=true&bindings_inherit=strict"
        } else {
            "bindings_inherit=strict"
        };
        Request::builder()
            .method("PUT")
            .uri(format!(
                "/client/v4/accounts/{public_account}/workers/scripts/{name}?{query}"
            ))
            .header("authorization", "Bearer admin-token")
            .header("content-type", "multipart/form-data; boundary=fixed")
            .body(Body::from(body.clone()))
            .unwrap()
    };
    let cross_script = app
        .clone()
        .oneshot(upload_script("other-worker", false))
        .await
        .unwrap();
    assert!(!cross_script.status().is_success());

    let excluded_projection = app
        .clone()
        .oneshot(upload_script("assets-worker", true))
        .await
        .unwrap();
    assert_eq!(
        excluded_projection.status(),
        StatusCode::INTERNAL_SERVER_ERROR,
        "excludeScript is a response projection and must reach normal runtime validation"
    );

    let failed = app
        .clone()
        .oneshot(upload_script("assets-worker", false))
        .await
        .unwrap();
    assert!(!failed.status().is_success());
    let retried = app
        .clone()
        .oneshot(upload_script("assets-worker", false))
        .await
        .unwrap();
    assert_eq!(retried.status(), failed.status());

    let listed = app
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/client/v4/accounts/{public_account}/workers/scripts"
                ))
                .header("authorization", "Bearer read-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(listed.status(), StatusCode::OK);
    let listed_body = axum::body::to_bytes(listed.into_body(), 64 * 1024)
        .await
        .unwrap();
    let listed_json: serde_json::Value = serde_json::from_slice(&listed_body).unwrap();
    assert_eq!(listed_json["result"], serde_json::json!([]));
}
