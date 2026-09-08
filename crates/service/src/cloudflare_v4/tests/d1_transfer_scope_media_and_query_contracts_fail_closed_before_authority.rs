use super::*;

#[tokio::test]
async fn d1_transfer_scope_media_and_query_contracts_fail_closed_before_authority() {
    let (state, authority) = state();
    let base = format!(
        "/accounts/{}/d1/database/00000000000000000000000000000000",
        authority.public_id()
    );
    let read_only = app(state.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("{base}/export"))
                .header(header::AUTHORIZATION, "Bearer read-token")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"output_format":"polling"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(read_only.status(), StatusCode::FORBIDDEN);
    assert_eq!(json(read_only).await["errors"][0]["code"], 9_100_002);

    let duplicate_media = app(state.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("{base}/import"))
                .header(header::AUTHORIZATION, "Bearer deployer-token")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::CONTENT_TYPE, "application/json; charset=utf-8")
                .body(Body::from(r#"{"action":"init","etag":"00"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(duplicate_media.status(), StatusCode::BAD_REQUEST);

    let duplicate_timestamp = app(state.clone())
        .oneshot(
            Request::builder()
                .uri(format!(
                    "{base}/time_travel/bookmark?timestamp=2026-01-01T00%3A00%3A00Z&timestamp=2026-01-02T00%3A00%3A00Z"
                ))
                .header(header::AUTHORIZATION, "Bearer read-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(duplicate_timestamp.status(), StatusCode::BAD_REQUEST);

    let ambiguous_restore = app(state)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "{base}/time_travel/restore?bookmark=opaque&timestamp=2026-01-01T00%3A00%3A00Z"
                ))
                .header(header::AUTHORIZATION, "Bearer deployer-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(ambiguous_restore.status(), StatusCode::BAD_REQUEST);
}
