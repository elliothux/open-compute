use super::*;

#[tokio::test]
async fn backup_routes_authenticate_before_parsing_and_reject_get_bodies() {
    let (state, authority) = state();
    let path = format!(
        "/accounts/{}/open-compute/kv/namespaces/00000000000000000000000000000000/backups",
        authority.public_id()
    );
    let unauthenticated = app(state.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(&path)
                .header("idempotency-key", "has space")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(unauthenticated.status(), StatusCode::UNAUTHORIZED);

    let malformed = app(state.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(&path)
                .header(header::AUTHORIZATION, "Bearer admin-token")
                .header("idempotency-key", "has space")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(malformed.status(), StatusCode::BAD_REQUEST);

    let duplicate_idempotency = app(state.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(&path)
                .header(header::AUTHORIZATION, "Bearer admin-token")
                .header("idempotency-key", "first")
                .header("idempotency-key", "second")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(duplicate_idempotency.status(), StatusCode::BAD_REQUEST);

    let restore_path = format!(
        "/accounts/{}/open-compute/kv/backups/backup/restore",
        authority.public_id()
    );
    let duplicate_content_type = app(state.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(restore_path)
                .header(header::AUTHORIZATION, "Bearer admin-token")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::CONTENT_TYPE, "application/json; charset=utf-8")
                .body(Body::from(r#"{"name":"restored"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(duplicate_content_type.status(), StatusCode::BAD_REQUEST);

    let get_with_body = app(state)
        .oneshot(
            Request::builder()
                .uri(path)
                .header(header::AUTHORIZATION, "Bearer read-token")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(get_with_body.status(), StatusCode::BAD_REQUEST);
}
