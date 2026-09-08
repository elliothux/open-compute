use super::*;

#[tokio::test]
async fn authentication_is_fail_closed_and_all_responses_have_request_ids() {
    let (state, _) = state();
    let unauthenticated = app(state.clone())
        .oneshot(Request::builder().uri("/user").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(unauthenticated.status(), StatusCode::UNAUTHORIZED);
    assert!(unauthenticated.headers().contains_key(REQUEST_ID_HEADER));
    let body = json(unauthenticated).await;
    assert_eq!(body["success"], false);
    assert_eq!(body["result"], serde_json::Value::Null);

    for token in ["admin-token", "deployer-token", "read-token"] {
        let response = app(state.clone())
            .oneshot(
                Request::builder()
                    .uri("/user/tokens/verify")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(response.headers().contains_key(REQUEST_ID_HEADER));
        assert_eq!(json(response).await["result"]["status"], "active");
    }
}
