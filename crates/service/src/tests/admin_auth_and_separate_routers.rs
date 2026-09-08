use super::*;

#[tokio::test]
async fn admin_auth_and_separate_routers() {
    let health = HealthCoordinator::new();
    let state = test_state(health, Some("s3cret-token"));
    let public = http::public_router(state.clone());
    let admin = http::admin_router(state);

    let res = public
        .oneshot(
            Request::builder()
                .uri("/client/v4/open-compute/system/status")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);

    let res = admin
        .clone()
        .oneshot(
            Request::builder()
                .uri("/client/v4/open-compute/system/status")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);

    let res = admin
        .clone()
        .oneshot(
            Request::builder()
                .uri("/client/v4/open-compute/system/status")
                .header("Authorization", "Bearer s3cret-token")
                .header("x-forwarded-for", "1.1.1.1")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = axum::body::to_bytes(res.into_body(), 16_384).await.unwrap();
    let text = String::from_utf8(body.to_vec()).unwrap();
    assert!(!text.contains("pid"));
    assert!(!text.contains("pgid"));
    assert!(!text.contains("token"));

    let res = admin
        .oneshot(
            Request::builder()
                .uri("/client/v4/open-compute/system/status")
                .header("Authorization", "Bearer wrong-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}
