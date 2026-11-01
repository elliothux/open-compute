use super::*;
use axum::body::Body;
use axum::http::Request;

#[tokio::test]
async fn private_ingest_rejects_size_and_payload_failures() {
    let (_temporary, _mock, state, _account, _storage) =
        crate::tests::initialized_worker_http_fixture().await;
    let service = state.worker_api().unwrap().observability().unwrap().clone();

    let state = BackendState { service };

    let declared = ingest(
        State(state.clone()),
        Request::builder()
            .header(header::CONTENT_LENGTH, MAX_BODY + 1)
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(declared.status(), StatusCode::PAYLOAD_TOO_LARGE);

    for (body, expected) in [
        (Body::from("{}"), StatusCode::UNPROCESSABLE_ENTITY),
        (
            Body::from(vec![b'x'; MAX_BODY + 1]),
            StatusCode::PAYLOAD_TOO_LARGE,
        ),
    ] {
        let response = ingest(State(state.clone()), Request::builder().body(body).unwrap()).await;
        assert_eq!(response.status(), expected);
    }
    assert_eq!(unavailable("test").code(), ErrorCode::RuntimeUnavailable);
}
