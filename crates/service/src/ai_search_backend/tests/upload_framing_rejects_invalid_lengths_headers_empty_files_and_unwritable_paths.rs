use super::*;

#[tokio::test]
async fn upload_framing_rejects_invalid_lengths_headers_empty_files_and_unwritable_paths() {
    let valid = json!({
        "schemaVersion": 1,
        "name": "guide.txt",
        "contentType": "text/plain",
        "options": {},
    });
    for (name, bytes, code) in [
        (
            "zero-header",
            0_u32.to_be_bytes().to_vec(),
            ErrorCode::BindingLimitExceeded,
        ),
        (
            "oversized-header",
            u32::try_from(MAX_FRAME_METADATA_BYTES + 1)
                .unwrap()
                .to_be_bytes()
                .to_vec(),
            ErrorCode::BindingLimitExceeded,
        ),
        (
            "invalid-json",
            upload_frame(&Value::String("not an object".to_owned()), b"body"),
            ErrorCode::BindingProtocolError,
        ),
        (
            "wrong-version",
            upload_frame(
                &json!({
                    "schemaVersion": 2,
                    "name": "guide.txt",
                    "contentType": "text/plain",
                    "options": {},
                }),
                b"body",
            ),
            ErrorCode::BindingProtocolError,
        ),
        (
            "empty-body",
            upload_frame(&valid, b""),
            ErrorCode::BindingLimitExceeded,
        ),
    ] {
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().join(name);
        let error = stage_upload(Body::from(bytes), path.clone(), MAX_UPLOAD_BYTES as u64)
            .await
            .unwrap_err();
        assert_eq!(error.code(), code);
        assert!(!path.exists());
    }

    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("missing-parent/upload");
    let error = stage_upload(
        Body::from(upload_frame(&valid, b"body")),
        path.clone(),
        MAX_UPLOAD_BYTES as u64,
    )
    .await
    .unwrap_err();
    assert_eq!(error.code(), ErrorCode::ResourceUnavailable);
    assert!(!path.exists());
}
