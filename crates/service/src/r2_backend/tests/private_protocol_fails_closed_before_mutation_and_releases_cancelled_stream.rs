use super::*;

#[tokio::test]
async fn private_protocol_fails_closed_before_mutation_and_releases_cancelled_stream() {
    let fixture = fixture().await;
    let bucket = R2BucketRepository::new(fixture.storage.db())
        .get(
            fixture.storage.identity().default_account_id,
            fixture.resource,
        )
        .unwrap();
    fixture.mock.put_raw(
        &format!(
            "{}objects/{}",
            bucket.physical_prefix,
            hex::encode(Sha256::digest(b"provider-only"))
        ),
        b"unowned".to_vec(),
    );
    for _ in 0..2 {
        let provider_only = fixture
            .service
            .handle(request(
                &fixture,
                "head",
                JSON_CONTENT_TYPE,
                Body::from(r#"{"key":"provider-only"}"#),
            ))
            .await;
        assert_eq!(provider_only.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(
            provider_only.headers().get(ERROR_HEADER).unwrap(),
            ErrorCode::R2ObjectMetadataInvalid.as_str()
        );
    }
    let bad_md5 = fixture
        .service
        .handle(request(
            &fixture,
            "put",
            FRAME_CONTENT_TYPE,
            put_frame("key", b"value", serde_json::json!({"md5": [0, 1]})),
        ))
        .await;
    assert_eq!(bad_md5.status(), StatusCode::BAD_REQUEST);

    let wrong_descriptor = axum::extract::Request::builder()
        .method("POST")
        .uri(format!("/internal/bindings/v1/r2/{}/head", fixture.binding))
        .header("content-type", JSON_CONTENT_TYPE)
        .header("x-open-compute-version-id", fixture.version.to_string())
        .header("x-open-compute-descriptor-sha256", "00".repeat(32))
        .header(
            "x-open-compute-request-id",
            uuid::Uuid::now_v7().hyphenated().to_string(),
        )
        .body(Body::from(r#"{"key":"key"}"#))
        .unwrap();
    let rejected = fixture.service.handle(wrong_descriptor).await;
    assert_eq!(rejected.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(fixture.pins.count(fixture.resource), 0);
}
