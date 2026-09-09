use super::*;

#[tokio::test]
async fn bodyless_vendor_posts_reject_content() {
    let (state, _) = state();
    let response = app(state)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/open-compute/scheduler/pause")
                .header(header::AUTHORIZATION, "Bearer admin-token")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(json(response).await["errors"][0]["code"], 9_100_003);
}
