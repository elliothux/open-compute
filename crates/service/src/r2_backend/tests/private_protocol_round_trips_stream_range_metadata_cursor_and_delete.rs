use super::*;

#[tokio::test]
async fn private_protocol_round_trips_stream_range_metadata_cursor_and_delete() {
    let fixture = fixture().await;
    let put = fixture
        .service
        .handle(request(
            &fixture,
            "put",
            FRAME_CONTENT_TYPE,
            put_frame(
                "folder/a + %.txt",
                b"hello world",
                serde_json::json!({
                    "httpMetadata": {"contentType": "text/plain"},
                    "customMetadata": {"author": "Elliot"},
                    "checksum": {
                        "algorithm": "md5",
                        "hex": "5eb63bbbe01eeed093cb22bb8f5acdc3"
                    },
                    "storageClass": "Standard"
                }),
            ),
        ))
        .await;
    assert_eq!(put.status(), StatusCode::OK);
    let put = body_json(put).await;
    assert_eq!(put["size"], 11);
    assert_eq!(put["customMetadata"]["author"], "Elliot");
    assert!(
        std::fs::read_dir(fixture.storage.data_dir().root().join("r2-staging"))
            .unwrap()
            .next()
            .is_none()
    );

    let head = fixture
        .service
        .handle(request(
            &fixture,
            "head",
            JSON_CONTENT_TYPE,
            Body::from(r#"{"key":"folder/a + %.txt"}"#),
        ))
        .await;
    assert_eq!(head.status(), StatusCode::OK);
    assert_eq!(
        body_json(head).await["httpMetadata"]["contentType"],
        "text/plain"
    );

    let get = fixture
        .service
        .handle(request(
            &fixture,
            "get",
            FRAME_CONTENT_TYPE,
            Body::from(r#"{"key":"folder/a + %.txt","options":{"range":{"offset":6,"length":5}}}"#),
        ))
        .await;
    assert_eq!(fixture.pins.count(fixture.resource), 1);
    let frame = to_bytes(get.into_body(), 1024 * 1024).await.unwrap();
    assert_eq!(fixture.pins.count(fixture.resource), 0);
    let header_len = u32::from_be_bytes(frame[..4].try_into().unwrap()) as usize;
    let metadata: serde_json::Value = serde_json::from_slice(&frame[4..4 + header_len]).unwrap();
    assert_eq!(metadata["meta"]["size"], 11);
    assert_eq!(metadata["meta"]["range"]["length"], 5);
    assert_eq!(&frame[4 + header_len..], b"world");

    let second = fixture
        .service
        .handle(request(
            &fixture,
            "put",
            FRAME_CONTENT_TYPE,
            put_frame("folder/b", b"second", serde_json::json!({})),
        ))
        .await;
    assert_eq!(second.status(), StatusCode::OK);
    let list = fixture
        .service
        .handle(request(
            &fixture,
            "list",
            JSON_CONTENT_TYPE,
            Body::from(r#"{"prefix":"folder/","limit":1,"include":[]}"#),
        ))
        .await;
    let list = body_json(list).await;
    assert!(list["truncated"].as_bool().unwrap());
    let cursor = list["cursor"].as_str().unwrap();
    assert!(!cursor.contains("folder"));
    let next = fixture
        .service
        .handle(request(
            &fixture,
            "list",
            JSON_CONTENT_TYPE,
            Body::from(
                serde_json::to_vec(&serde_json::json!({
                    "prefix": "folder/",
                    "limit": 1,
                    "include": [],
                    "cursor": cursor
                }))
                .unwrap(),
            ),
        ))
        .await;
    assert_eq!(next.status(), StatusCode::OK);

    let tampered = format!("{cursor}x");
    let invalid = fixture
        .service
        .handle(request(
            &fixture,
            "list",
            JSON_CONTENT_TYPE,
            Body::from(
                serde_json::to_vec(&serde_json::json!({
                    "prefix": "folder/",
                    "limit": 1,
                    "include": [],
                    "cursor": tampered
                }))
                .unwrap(),
            ),
        ))
        .await;
    assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        invalid.headers().get(ERROR_HEADER).unwrap(),
        ErrorCode::R2CursorInvalid.as_str()
    );

    let deleted = fixture
        .service
        .handle(request(
            &fixture,
            "delete",
            JSON_CONTENT_TYPE,
            Body::from(r#"{"keys":["folder/a + %.txt","folder/b"]}"#),
        ))
        .await;
    assert_eq!(deleted.status(), StatusCode::NO_CONTENT);
}
