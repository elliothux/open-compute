use super::*;

#[tokio::test]
async fn get_after_put_returns_object_body_via_authoritative_head() {
    let fixture = fixture().await;
    let key = "authority-get-small";
    let put = fixture
        .service
        .handle(request(
            &fixture,
            "put",
            FRAME_CONTENT_TYPE,
            put_frame(key, b"payload-bytes", serde_json::json!({})),
        ))
        .await;
    assert_eq!(put.status(), StatusCode::OK);

    let get = fixture
        .service
        .handle(request(
            &fixture,
            "get",
            FRAME_CONTENT_TYPE,
            Body::from(format!(r#"{{"key":"{key}","options":{{}}}}"#)),
        ))
        .await;
    assert_eq!(get.status(), StatusCode::OK);
    let frame = to_bytes(get.into_body(), 1024 * 1024).await.unwrap();
    let header_len = u32::from_be_bytes(frame[..4].try_into().unwrap()) as usize;
    assert_eq!(&frame[4 + header_len..], b"payload-bytes");
}
