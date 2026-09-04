use super::*;
use axum::body::Body;
use axum::http::Request;
use open_compute_core::SecretString;

#[tokio::test]
async fn private_ingest_rejects_size_authentication_and_payload_failures() {
    let (_temporary, _mock, state, _account, _storage) =
        crate::tests::initialized_worker_http_fixture().await;
    let service = state.worker_api().unwrap().observability().unwrap().clone();
    let auth = GenerationAuthRegistry::new();
    let token = "ab".repeat(32);
    auth.activate_for_test(SecretString::new(&token));
    let state = BackendState { service, auth };

    let declared = ingest(
        State(state.clone()),
        Request::builder()
            .header(header::CONTENT_LENGTH, MAX_BODY + 1)
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(declared.status(), StatusCode::PAYLOAD_TOO_LARGE);

    let unauthenticated = ingest(State(state.clone()), Request::new(Body::empty())).await;
    assert_eq!(unauthenticated.status(), StatusCode::NOT_FOUND);

    for (body, expected) in [
        (Body::from("{}"), StatusCode::UNPROCESSABLE_ENTITY),
        (
            Body::from(vec![b'x'; MAX_BODY + 1]),
            StatusCode::PAYLOAD_TOO_LARGE,
        ),
    ] {
        let response = ingest(
            State(state.clone()),
            Request::builder()
                .header(TOKEN_HEADER, &token)
                .header(GENERATION_HEADER, "generation")
                .body(body)
                .unwrap(),
        )
        .await;
        assert_eq!(response.status(), expected);
    }
    assert_eq!(unavailable("test").code(), ErrorCode::RuntimeUnavailable);
}
