use super::*;

#[tokio::test]
async fn private_protocol_covers_checksum_ssec_storage_class_multipart_and_start_after() {
    let fixture = fixture().await;
    let ssec = "ab".repeat(32);
    let mismatch = fixture
        .service
        .handle(request(
            &fixture,
            "put",
            FRAME_CONTENT_TYPE,
            put_frame(
                "bad-md5",
                b"hello",
                serde_json::json!({"checksum": {"algorithm": "md5", "hex": "00".repeat(16)}}),
            ),
        ))
        .await;
    assert_eq!(mismatch.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        mismatch.headers().get(ERROR_HEADER).unwrap(),
        ErrorCode::R2ChecksumMismatch.as_str()
    );

    let ia = fixture
        .service
        .handle(request(
            &fixture,
            "put",
            FRAME_CONTENT_TYPE,
            put_frame(
                "ia.bin",
                b"ia",
                serde_json::json!({"storageClass": "InfrequentAccess"}),
            ),
        ))
        .await;
    assert_eq!(ia.status(), StatusCode::OK);
    assert_eq!(body_json(ia).await["storageClass"], "InfrequentAccess");

    let ssec_put = fixture
        .service
        .handle(request(
            &fixture,
            "put",
            FRAME_CONTENT_TYPE,
            put_frame(
                "secret.bin",
                b"secret",
                serde_json::json!({"ssecKey": ssec}),
            ),
        ))
        .await;
    assert_eq!(ssec_put.status(), StatusCode::OK);
    let ssec_put = body_json(ssec_put).await;
    assert!(!ssec_put["ssecKeyMd5"].as_str().unwrap().is_empty());
    let restarted = R2BindingService::new(
        fixture.storage.clone(),
        fixture.pins.clone(),
        fixture.objects.clone(),
        R2Config {
            max_object_bytes: 1024 * 1024,
            max_staging_bytes: 2 * 1024 * 1024,
            operation_timeout_ms: 1000,
            ..R2Config::default()
        },
    )
    .unwrap();
    let ssec_head = restarted
        .handle(request(
            &fixture,
            "head",
            JSON_CONTENT_TYPE,
            Body::from(r#"{"key":"secret.bin"}"#),
        ))
        .await;
    assert_eq!(ssec_head.status(), StatusCode::OK);
    assert_eq!(
        body_json(ssec_head).await["ssecKeyMd5"],
        ssec_put["ssecKeyMd5"]
    );
    let denied = fixture
        .service
        .handle(request(
            &fixture,
            "get",
            FRAME_CONTENT_TYPE,
            Body::from(r#"{"key":"secret.bin","options":{}}"#),
        ))
        .await;
    assert_eq!(denied.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        denied.headers().get(ERROR_HEADER).unwrap(),
        ErrorCode::R2SsecInvalid.as_str()
    );
    let allowed = fixture
        .service
        .handle(request(
            &fixture,
            "get",
            FRAME_CONTENT_TYPE,
            Body::from(
                serde_json::json!({"key":"secret.bin","options":{"ssecKey": ssec}}).to_string(),
            ),
        ))
        .await;
    assert_eq!(allowed.status(), StatusCode::OK);

    let skipped = fixture
        .service
        .handle(request(
            &fixture,
            "put",
            FRAME_CONTENT_TYPE,
            put_frame(
                "ia.bin",
                b"nope",
                serde_json::json!({"onlyIf":{"etagMatches":[{"kind":"strong","value":"missing"}]}}),
            ),
        ))
        .await;
    assert_eq!(skipped.status(), StatusCode::NO_CONTENT);

    let listed = fixture
        .service
        .handle(request(
            &fixture,
            "list",
            JSON_CONTENT_TYPE,
            Body::from(r#"{"prefix":"","limit":1000,"include":[],"startAfter":"ia.bin"}"#),
        ))
        .await;
    let listed = body_json(listed).await;
    assert!(
        listed["objects"]
            .as_array()
            .unwrap()
            .iter()
            .all(|object| object["key"] != "ia.bin")
    );
    assert!(
        listed["objects"]
            .as_array()
            .unwrap()
            .iter()
            .any(|object| object["key"] == "secret.bin")
    );
    let ssec_md5 = ssec_put["ssecKeyMd5"].as_str().unwrap();
    let provider_ssec_md5 =
        base64::engine::general_purpose::STANDARD.encode(hex::decode(ssec_md5).unwrap());
    let physical_suffix = hex::encode(Sha256::digest(b"secret.bin"));
    assert!(fixture.mock.recorded().iter().any(|request| {
        request.method == "HEAD"
            && request.path.ends_with(&physical_suffix)
            && request.ssec_algorithm.as_deref() == Some("AES256")
            && request.ssec_key_md5.as_deref() == Some(provider_ssec_md5.as_str())
    }));

    let created = fixture
        .service
        .handle(request(
            &fixture,
            "createMultipartUpload",
            JSON_CONTENT_TYPE,
            Body::from(
                serde_json::json!({
                    "key": "mpu.txt",
                    "options": {"storageClass": "Standard", "ssecKey": ssec}
                })
                .to_string(),
            ),
        ))
        .await;
    assert_eq!(created.status(), StatusCode::OK);
    let created = body_json(created).await;
    let upload_id = created["uploadId"].as_str().unwrap().to_owned();
    let part = fixture
        .service
        .handle(request(
            &fixture,
            "uploadPart",
            FRAME_CONTENT_TYPE,
            part_frame("mpu.txt", &upload_id, 1, b"part-body", Some(&ssec)),
        ))
        .await;
    assert_eq!(part.status(), StatusCode::OK);
    let part = body_json(part).await;
    fixture
        .mock
        .set_fault(open_compute_artifacts::Fault::CompleteResponseLoss);
    let completed = fixture
        .service
        .handle(request(
            &fixture,
            "completeMultipartUpload",
            JSON_CONTENT_TYPE,
            Body::from(
                serde_json::json!({
                    "key": "mpu.txt",
                    "uploadId": upload_id,
                    "parts": [{"partNumber": 1, "etag": part["etag"]}]
                })
                .to_string(),
            ),
        ))
        .await;
    assert_eq!(completed.status(), StatusCode::OK);
    let completed = body_json(completed).await;
    assert_eq!(completed["key"], "mpu.txt");
    fixture.mock.set_fault(open_compute_artifacts::Fault::None);

    let other = fixture
        .service
        .handle(request(
            &fixture,
            "createMultipartUpload",
            JSON_CONTENT_TYPE,
            Body::from(r#"{"key":"abort.txt","options":{}}"#),
        ))
        .await;
    let other_id = body_json(other).await["uploadId"]
        .as_str()
        .unwrap()
        .to_owned();
    let aborted = fixture
        .service
        .handle(request(
            &fixture,
            "abortMultipartUpload",
            JSON_CONTENT_TYPE,
            Body::from(serde_json::json!({"key":"abort.txt","uploadId": other_id}).to_string()),
        ))
        .await;
    assert_eq!(aborted.status(), StatusCode::NO_CONTENT);
    let raced = fixture
        .service
        .handle(request(
            &fixture,
            "completeMultipartUpload",
            JSON_CONTENT_TYPE,
            Body::from(
                serde_json::json!({
                    "key": "mpu.txt",
                    "uploadId": upload_id,
                    "parts": [{"partNumber": 1, "etag": part["etag"]}]
                })
                .to_string(),
            ),
        ))
        .await;
    assert_eq!(raced.status(), StatusCode::OK);
    assert_eq!(body_json(raced).await, completed);

    let conflicting = fixture
        .service
        .handle(request(
            &fixture,
            "completeMultipartUpload",
            JSON_CONTENT_TYPE,
            Body::from(
                serde_json::json!({
                    "key": "mpu.txt",
                    "uploadId": upload_id,
                    "parts": []
                })
                .to_string(),
            ),
        ))
        .await;
    assert_eq!(conflicting.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        conflicting.headers().get(ERROR_HEADER).unwrap(),
        ErrorCode::R2MultipartInvalid.as_str()
    );

    rusqlite::Connection::open(fixture._temp.path().join("data/control.sqlite"))
        .unwrap()
        .execute(
            "UPDATE r2_multipart_uploads SET completion_manifest = ?1 WHERE upload_id = ?2",
            rusqlite::params![r#"[{"partNumber":1,"etag":"forged"}]"#, upload_id],
        )
        .unwrap();
    let forged = fixture
        .service
        .handle(request(
            &fixture,
            "completeMultipartUpload",
            JSON_CONTENT_TYPE,
            Body::from(
                serde_json::json!({
                    "key": "mpu.txt",
                    "uploadId": upload_id,
                    "parts": [{"partNumber": 1, "etag": "forged"}]
                })
                .to_string(),
            ),
        ))
        .await;
    assert_eq!(forged.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        forged.headers().get(ERROR_HEADER).unwrap(),
        ErrorCode::R2MultipartInvalid.as_str()
    );
}
