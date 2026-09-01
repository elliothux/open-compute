use super::*;
use axum::http::HeaderMap;

#[tokio::test]
async fn wire_validation_and_error_mapping_cover_every_bounded_shape() {
    let unsupported: PutWireOptions =
        serde_json::from_value(serde_json::json!({"storageClass": "Archive"})).unwrap();
    assert_eq!(
        unsupported.validate().unwrap_err().code(),
        ErrorCode::R2InvalidOptions
    );
    let infrequent: R2PutOptions = serde_json::from_value::<PutWireOptions>(
        serde_json::json!({"storageClass": "InfrequentAccess"}),
    )
    .unwrap()
    .try_into()
    .unwrap();
    assert_eq!(infrequent.storage_class, R2StorageClass::InfrequentAccess);
    let date_condition: PutWireOptions = serde_json::from_value(serde_json::json!({
        "onlyIf": {"uploadedBefore": 1}
    }))
    .unwrap();
    assert!(date_condition.validate().is_ok());
    let hex_md5: R2PutOptions = serde_json::from_value::<PutWireOptions>(serde_json::json!({
        "checksum": {"algorithm": "md5", "hex": "00000000000000000000000000000000"}
    }))
    .unwrap()
    .try_into()
    .unwrap();
    assert!(matches!(
        hex_md5.checksum,
        Some(R2ChecksumAlgorithm::Md5(_))
    ));
    for invalid in [
        serde_json::json!({"checksum": {"algorithm": "md5", "hex": "zz"}}),
        serde_json::json!({"checksum": {"algorithm": "md4", "hex": "00".repeat(16)}}),
        serde_json::json!({"ssecKey": "abcd"}),
    ] {
        assert!(
            R2PutOptions::try_from(serde_json::from_value::<PutWireOptions>(invalid).unwrap())
                .is_err()
        );
    }

    let defaults: ListRequest = serde_json::from_slice(b"{}").unwrap();
    assert_eq!(defaults.limit, R2_MAX_LIST_LIMIT);
    assert!(
        serde_json::from_value::<ListRequest>(serde_json::json!({"limit": 0}))
            .unwrap()
            .validate()
            .is_ok()
    );
    for invalid in [
        serde_json::json!({"limit": 1001}),
        serde_json::json!({"delimiter": ""}),
        serde_json::json!({"include": ["a", "b", "c"]}),
    ] {
        assert_eq!(
            serde_json::from_value::<ListRequest>(invalid)
                .unwrap()
                .validate()
                .unwrap_err()
                .code(),
            ErrorCode::R2InvalidOptions
        );
    }
    assert_eq!(
        include_mask(&["httpMetadata".to_owned(), "customMetadata".to_owned()]).unwrap(),
        3
    );
    assert_eq!(
        include_mask(&["unknown".to_owned()]).unwrap_err().code(),
        ErrorCode::R2InvalidOptions
    );
    let listed = list_object_json(
        open_compute_artifacts::R2ObjectMetadata {
            key: "k".to_owned(),
            version: uuid::Uuid::now_v7().to_string(),
            size: 1,
            etag: "e".to_owned(),
            http_etag: "\"e\"".to_owned(),
            uploaded: 2,
            http_metadata: Some(R2HttpMetadata::default()),
            custom_metadata: Some(Default::default()),
            range: None,
            checksums: open_compute_artifacts::R2Checksums::default(),
            storage_class: "Standard".to_owned(),
            ssec_key_md5: None,
        },
        0,
    );
    assert_eq!(listed["httpEtag"], "\"e\"");
    assert!(listed.get("httpMetadata").is_none());

    let binding = BindingId::generate();
    for operation in [
        "head",
        "get",
        "put",
        "delete",
        "list",
        "createMultipartUpload",
        "uploadPart",
        "completeMultipartUpload",
        "abortMultipartUpload",
    ] {
        assert!(parse_path(&format!("/internal/bindings/v1/r2/{binding}/{operation}")).is_ok());
    }
    for path in [
        "bad",
        "/internal/bindings/v1/r2/bad/head",
        &format!("/internal/bindings/v1/r2/{binding}/unknown"),
        &format!("/internal/bindings/v1/r2/{binding}/head/extra"),
    ] {
        assert_eq!(
            parse_path(path).unwrap_err().code(),
            ErrorCode::BindingProtocolError
        );
    }

    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(JSON_CONTENT_TYPE),
    );
    assert!(content_type_matches(&headers, Operation::Head));
    assert!(!content_type_matches(&headers, Operation::Get));
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/vnd.open-compute.r2.v1+frame; charset=binary"),
    );
    assert!(content_type_matches(&headers, Operation::Put));
    assert!(parse_header::<u64>(&headers, "missing").is_err());
    headers.insert("number", HeaderValue::from_static("42"));
    assert_eq!(parse_header::<u64>(&headers, "number").unwrap(), 42);
    assert!(parse_digest(&headers).is_err());
    headers.insert(
        "x-open-compute-descriptor-sha256",
        HeaderValue::from_static("00"),
    );
    assert!(parse_digest(&headers).is_err());
    headers.insert(
        "x-open-compute-descriptor-sha256",
        HeaderValue::from_str(&"00".repeat(32)).unwrap(),
    );
    assert_eq!(parse_digest(&headers).unwrap(), [0; 32]);
    assert!(parse_request_id(&headers).is_err());
    headers.insert(
        "x-open-compute-request-id",
        HeaderValue::from_static("not-a-uuid"),
    );
    assert!(parse_request_id(&headers).is_err());
    let request_id = uuid::Uuid::now_v7().hyphenated().to_string();
    headers.insert(
        "x-open-compute-request-id",
        HeaderValue::from_str(&request_id.to_uppercase()).unwrap(),
    );
    assert!(parse_request_id(&headers).is_err());
    headers.insert(
        "x-open-compute-request-id",
        HeaderValue::from_str(&request_id).unwrap(),
    );
    assert_eq!(parse_request_id(&headers).unwrap(), request_id);

    assert_eq!(
        bounded_json(Body::from("too large"), 1)
            .await
            .unwrap_err()
            .code(),
        ErrorCode::BindingProtocolError
    );
    assert!(parse_json::<KeyRequest>(b"{").is_err());
    assert_eq!(
        timeout_result(Duration::from_millis(1), async {
            tokio::time::sleep(Duration::from_millis(20)).await;
            Ok::<_, PlatformError>(())
        })
        .await
        .unwrap_err()
        .code(),
        ErrorCode::R2ProviderUnavailable
    );
    assert_eq!(
        mutation_timeout_result(Duration::from_millis(1), async {
            tokio::time::sleep(Duration::from_millis(20)).await;
            Ok::<_, PlatformError>(())
        })
        .await
        .unwrap_err()
        .code(),
        ErrorCode::R2ResultUnknown
    );
    assert_eq!(digest_text("x").len(), 64);
    assert!(unix_ms().unwrap() > 0);

    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("data");
    let storage = PlatformStorage::bootstrap(
        &open_compute_core::config::StorageConfig {
            data_dir: root.clone(),
            master_key_file: root.join("keys/master.key"),
            master_key_env: None,
            sqlite_busy_timeout_ms: 5_000,
            free_space_soft_bytes: 1,
            free_space_hard_bytes: 1,
        },
        &open_compute_core::SystemClock,
    )
    .unwrap();
    assert_eq!(
        ensure_storage_headroom(&storage, u64::MAX)
            .unwrap_err()
            .code(),
        ErrorCode::R2Overloaded
    );

    let cases = [
        (ErrorCode::ResourceNotFound, StatusCode::NOT_FOUND),
        (ErrorCode::BindingPermissionDenied, StatusCode::FORBIDDEN),
        (ErrorCode::R2ObjectTooLarge, StatusCode::PAYLOAD_TOO_LARGE),
        (ErrorCode::ResourceNotReady, StatusCode::CONFLICT),
        (ErrorCode::R2Overloaded, StatusCode::SERVICE_UNAVAILABLE),
        (
            ErrorCode::R2ObjectMetadataInvalid,
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (ErrorCode::R2InvalidOptions, StatusCode::BAD_REQUEST),
        (ErrorCode::Internal, StatusCode::INTERNAL_SERVER_ERROR),
    ];
    for (code, status) in cases {
        let response = error_response(&PlatformError::new(code, "safe"));
        assert_eq!(response.status(), status);
        assert_eq!(response.headers().get(ERROR_HEADER).unwrap(), code.as_str());
    }
    assert!(metric_operation("/unknown").is_none());
    for error in [
        protocol_error(),
        metadata_too_large(),
        metadata_invalid(),
        cursor_invalid(),
        object_too_large(),
        overloaded(),
    ] {
        assert!(!error.code().as_str().is_empty());
    }
}
