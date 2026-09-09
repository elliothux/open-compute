use super::*;

#[tokio::test]
async fn account_collections_use_public_ids_and_sibling_result_info() {
    let (state, authority) = state();
    let response = app(state.clone())
        .oneshot(
            Request::builder()
                .uri("/accounts?page=1&per_page=20")
                .header(header::AUTHORIZATION, "Bearer read-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = json(response).await;
    assert_eq!(body["result"][0]["id"], authority.public_id());
    assert_eq!(body["result_info"]["total_count"], 1);
    assert!(body["result"][0]["id"].as_str().unwrap().len() == 32);

    let detail = app(state)
        .oneshot(
            Request::builder()
                .uri(format!("/accounts/{}", authority.public_id()))
                .header(header::AUTHORIZATION, "Bearer deployer-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(detail.status(), StatusCode::OK);
    assert_eq!(json(detail).await["result"]["type"], "standard");
}
