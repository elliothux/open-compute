use super::*;

#[tokio::test]
async fn storage_boundaries_return_cloudflare_errors_before_domain_dispatch() {
    let (state, authority) = state();
    let invalid_query = app(state.clone())
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/accounts/{}/storage/kv/namespaces?page=1&page=2",
                    authority.public_id()
                ))
                .header(header::AUTHORIZATION, "Bearer read-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(invalid_query.status(), StatusCode::BAD_REQUEST);
    assert!(invalid_query.headers().contains_key(REQUEST_ID_HEADER));
    assert_eq!(json(invalid_query).await["errors"][0]["code"], 9_100_003);

    let forbidden_query = app(state.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/accounts/{}/storage/kv/namespaces?unknown=true",
                    authority.public_id()
                ))
                .header(header::AUTHORIZATION, "Bearer deployer-token")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"title":"namespace"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(forbidden_query.status(), StatusCode::BAD_REQUEST);
    assert!(forbidden_query.headers().contains_key(REQUEST_ID_HEADER));
    assert_eq!(json(forbidden_query).await["errors"][0]["code"], 9_100_003);

    let denied = app(state.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/accounts/{}/storage/kv/namespaces",
                    authority.public_id()
                ))
                .header(header::AUTHORIZATION, "Bearer read-token")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"title":"namespace"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);
    assert_eq!(json(denied).await["errors"][0]["code"], 9_100_002);

    let unsupported = app(state)
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/accounts/{}/r2/buckets/valid-bucket/objects",
                    authority.public_id()
                ))
                .header(header::AUTHORIZATION, "Bearer read-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(unsupported.status(), StatusCode::NOT_IMPLEMENTED);
    assert_eq!(json(unsupported).await["errors"][0]["code"], 9_100_007);
}
