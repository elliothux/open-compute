use super::*;

#[tokio::test]
async fn v4_account_subdomain_is_a_stable_read_only_unroutable_prerequisite() {
    let (_dir, _mock, state, account, _storage) = initialized_worker_http_fixture().await;
    let authority = crate::cloudflare_v4::accounts::AccountAuthority::new(
        open_compute_core::PlatformId::generate(),
        account,
        1_000,
    );
    let public_account = authority.public_id().to_owned();
    let path = format!("/client/v4/accounts/{public_account}/workers/subdomain");
    let app = http::admin_router(
        state
            .with_v4_tokens(
                SecretString::new("deployer-token"),
                SecretString::new("read-token"),
            )
            .with_cloudflare_v4_account(authority),
    );

    let get = || {
        Request::builder()
            .uri(&path)
            .header("authorization", "Bearer read-token")
            .body(Body::empty())
            .unwrap()
    };
    let first = app.clone().oneshot(get()).await.unwrap();
    assert_eq!(first.status(), StatusCode::OK);
    let first: serde_json::Value = serde_json::from_slice(
        &axum::body::to_bytes(first.into_body(), 64 * 1024)
            .await
            .unwrap(),
    )
    .unwrap();
    let label = first["result"]["subdomain"].as_str().unwrap();
    assert_eq!(label, format!("_open-compute-unroutable-{public_account}"));
    assert!(
        label.starts_with('_'),
        "the prerequisite must not be a DNS hostname"
    );

    let repeated = app.clone().oneshot(get()).await.unwrap();
    let repeated: serde_json::Value = serde_json::from_slice(
        &axum::body::to_bytes(repeated.into_body(), 64 * 1024)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(repeated["result"]["subdomain"], label);

    let missing = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/client/v4/accounts/00000000000000000000000000000000/workers/subdomain")
                .header("authorization", "Bearer read-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);

    for method in ["PUT", "DELETE"] {
        let mutation = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(&path)
                    .header("authorization", "Bearer admin-token")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"subdomain":"public"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(mutation.status(), StatusCode::METHOD_NOT_ALLOWED);
    }
}
