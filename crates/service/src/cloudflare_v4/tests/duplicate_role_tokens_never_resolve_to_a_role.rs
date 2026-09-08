use super::*;

#[tokio::test]
async fn duplicate_role_tokens_never_resolve_to_a_role() {
    let (state, authority) = state();
    let state = HttpState::for_test(
        HealthCoordinator::new(),
        state.metrics().clone(),
        false,
        Some(SecretString::new("same-token")),
    )
    .with_v4_tokens(
        SecretString::new("same-token"),
        SecretString::new("read-token"),
    )
    .with_cloudflare_v4_account(authority);
    let response = app(state)
        .oneshot(
            Request::builder()
                .uri("/user")
                .header(header::AUTHORIZATION, "Bearer same-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}
