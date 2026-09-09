use super::*;

#[tokio::test]
async fn permission_and_query_errors_never_use_authentication_code() {
    let (state, _) = state();
    let denied = app(state.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/open-compute/scheduler/pause")
                .header(header::AUTHORIZATION, "Bearer deployer-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);
    let denied_body = json(denied).await;
    assert_eq!(denied_body["errors"][0]["code"], 9_100_002);
    assert_ne!(denied_body["errors"][0]["code"], 10_000);

    let invalid = app(state)
        .oneshot(
            Request::builder()
                .uri("/accounts?per_page=1")
                .header(header::AUTHORIZATION, "Bearer read-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);
    let invalid_body = json(invalid).await;
    assert_eq!(invalid_body["errors"][0]["code"], 9_100_003);
    assert_ne!(invalid_body["errors"][0]["code"], 10_000);
}
