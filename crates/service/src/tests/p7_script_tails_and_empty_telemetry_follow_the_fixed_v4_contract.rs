use super::*;

#[tokio::test]
async fn p7_script_tails_and_empty_telemetry_follow_the_fixed_v4_contract() {
    let (_dir, _mock, state, account, storage) = initialized_worker_http_fixture().await;
    let repo = open_compute_storage::WorkerRepository::new(storage.db());
    repo.create_worker(
        account,
        "tail-worker",
        open_compute_core::RequestId::generate(),
        1,
        1_000_000,
    )
    .unwrap();
    let authority = crate::cloudflare_v4::accounts::AccountAuthority::new(
        open_compute_core::PlatformId::generate(),
        account,
        1_000,
    );
    let public_account = authority.public_id().to_owned();
    let app = http::admin_router(
        state
            .with_v4_tokens(
                SecretString::new("deployer-token"),
                SecretString::new("read-token"),
            )
            .with_cloudflare_v4_account(authority),
    );
    let tails_path =
        format!("/client/v4/accounts/{public_account}/workers/scripts/tail-worker/tails");
    let created = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(&tails_path)
                .header("authorization", "Bearer read-token")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"filters":[{"outcome":["ok"]},{"method":["get"]},{"query":"invoice"}]}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::OK);
    let created: serde_json::Value = serde_json::from_slice(
        &axum::body::to_bytes(created.into_body(), 64 * 1024)
            .await
            .unwrap(),
    )
    .unwrap();
    let tail_id = created["result"]["id"].as_str().unwrap().to_owned();
    assert!(
        created["result"]["url"]
            .as_str()
            .unwrap()
            .starts_with("ws://127.0.0.1:8787/client/v4/open-compute/tails/")
    );
    assert!(created["result"]["expires_at"].as_str().is_some());

    let listed = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(&tails_path)
                .header("authorization", "Bearer read-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let listed: serde_json::Value = serde_json::from_slice(
        &axum::body::to_bytes(listed.into_body(), 64 * 1024)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(listed["result"].as_array().unwrap().len(), 1);
    assert_eq!(listed["result"][0]["id"], tail_id);

    let deleted = app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("{tails_path}/{tail_id}"))
                .header("authorization", "Bearer read-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(deleted.status(), StatusCode::OK);

    let live_tail = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/client/v4/accounts/{public_account}/workers/observability/telemetry/live-tail"
                ))
                .header("authorization", "Bearer deployer-token")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"scriptId":"tail-worker","filterCombination":"and","filters":[{"key":"$workers.preview.slug","type":"string","operation":"is_null"}]}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(live_tail.status(), StatusCode::OK);
    let live_tail: serde_json::Value = serde_json::from_slice(
        &axum::body::to_bytes(live_tail.into_body(), 64 * 1024)
            .await
            .unwrap(),
    )
    .unwrap();
    assert!(
        live_tail["result"]["wsUrl"]
            .as_str()
            .unwrap()
            .starts_with("ws://127.0.0.1:8787/client/v4/open-compute/live-tails/")
    );
    let heartbeat = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/client/v4/accounts/{public_account}/workers/observability/telemetry/live-tail/heartbeat"
                ))
                .header("authorization", "Bearer deployer-token")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"scriptId":"tail-worker"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(heartbeat.status(), StatusCode::OK);
    let heartbeat: serde_json::Value = serde_json::from_slice(
        &axum::body::to_bytes(heartbeat.into_body(), 64 * 1024)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(heartbeat["result"], serde_json::json!({}));

    for _ in 0..9 {
        let admitted = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!(
                        "/client/v4/accounts/{public_account}/workers/observability/telemetry/live-tail"
                    ))
                    .header("authorization", "Bearer deployer-token")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"scriptId":"tail-worker"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(admitted.status(), StatusCode::OK);
    }
    let saturated = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/client/v4/accounts/{public_account}/workers/observability/telemetry/live-tail"
                ))
                .header("authorization", "Bearer deployer-token")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"scriptId":"tail-worker"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(saturated.status(), StatusCode::TOO_MANY_REQUESTS);

    let keys = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/client/v4/accounts/{public_account}/workers/observability/telemetry/keys"
                ))
                .header("authorization", "Bearer read-token")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"datasets":["cloudflare-workers"]}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(keys.status(), StatusCode::OK);
    let keys: serde_json::Value = serde_json::from_slice(
        &axum::body::to_bytes(keys.into_body(), 64 * 1024)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(keys["result"], serde_json::json!([]));
}
