use super::*;

#[tokio::test]
async fn provider_failures_are_secret_safe_and_mutation_response_loss_is_reconciled() {
    let fixture = fixture().await;
    let seeded = fixture
        .service
        .handle(request(
            &fixture,
            "put",
            FRAME_CONTENT_TYPE,
            put_frame("failure-key", b"abcdef", serde_json::json!({})),
        ))
        .await;
    assert_eq!(seeded.status(), StatusCode::OK);

    fixture
        .mock
        .set_fault(open_compute_artifacts::Fault::MidstreamReset);
    let interrupted = fixture
        .service
        .handle(request(
            &fixture,
            "get",
            FRAME_CONTENT_TYPE,
            Body::from(r#"{"key":"failure-key","options":{}}"#),
        ))
        .await;
    assert_eq!(interrupted.status(), StatusCode::OK);
    assert!(
        to_bytes(interrupted.into_body(), 1024 * 1024)
            .await
            .is_err()
    );
    assert_eq!(fixture.pins.count(fixture.resource), 0);

    fixture
        .mock
        .set_fault(open_compute_artifacts::Fault::PutResponseLoss);
    let unknown_put = fixture
        .service
        .handle(request(
            &fixture,
            "put",
            FRAME_CONTENT_TYPE,
            put_frame("lost-put", b"value", serde_json::json!({})),
        ))
        .await;
    assert_eq!(unknown_put.status(), StatusCode::OK);
    let body = to_bytes(unknown_put.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(body["key"], "lost-put");

    fixture.mock.set_fault(open_compute_artifacts::Fault::None);
    let for_delete = fixture
        .service
        .handle(request(
            &fixture,
            "put",
            FRAME_CONTENT_TYPE,
            put_frame("lost-delete", b"value", serde_json::json!({})),
        ))
        .await;
    assert_eq!(for_delete.status(), StatusCode::OK);
    fixture
        .mock
        .set_fault(open_compute_artifacts::Fault::DeleteResponseLoss);
    let recovered_delete = fixture
        .service
        .handle(request(
            &fixture,
            "delete",
            JSON_CONTENT_TYPE,
            Body::from(r#"{"keys":["lost-delete"]}"#),
        ))
        .await;
    assert_eq!(recovered_delete.status(), StatusCode::NO_CONTENT);
    let deleted = fixture
        .service
        .handle(request(
            &fixture,
            "head",
            JSON_CONTENT_TYPE,
            Body::from(r#"{"key":"lost-delete"}"#),
        ))
        .await;
    assert_eq!(deleted.status(), StatusCode::NO_CONTENT);

    fixture.mock.set_fault(open_compute_artifacts::Fault::Auth);
    let unavailable = fixture
        .service
        .handle(request(
            &fixture,
            "head",
            JSON_CONTENT_TYPE,
            Body::from(r#"{"key":"failure-key"}"#),
        ))
        .await;
    assert_eq!(unavailable.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        unavailable.headers().get(ERROR_HEADER).unwrap(),
        ErrorCode::R2ProviderUnavailable.as_str()
    );
}
