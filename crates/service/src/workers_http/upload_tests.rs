use super::*;
use axum::body::to_bytes;
use open_compute_workers::AssetEntryV1;

fn manifest(entries: Vec<AssetEntryV1>) -> AssetManifestV1 {
    AssetManifestV1 {
        schema_version: 1,
        entries,
    }
}

#[tokio::test]
async fn staged_asset_objects_are_exact_and_removed_after_use() {
    let temp = tempfile::tempdir().unwrap();
    let bytes = Bytes::from_static(b"asset bytes");
    let digest: [u8; 32] = Sha256::digest(&bytes).into();
    let staged = stage_object(
        Body::from(bytes.clone()),
        temp.path().to_path_buf(),
        &digest,
        bytes.len() as u64,
    )
    .await
    .unwrap();
    assert_eq!(std::fs::read(&staged.path).unwrap(), bytes);
    assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 1);
    drop(staged);
    assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 0);

    for (body, expected_digest, expected_size, expected_code) in [
        (
            Body::from(Bytes::from_static(b"too long")),
            digest,
            1,
            ErrorCode::AssetUploadConflict,
        ),
        (
            Body::from(Bytes::from_static(b"short")),
            digest,
            bytes.len() as u64,
            ErrorCode::AssetUploadConflict,
        ),
        (
            Body::from(bytes.clone()),
            [9; 32],
            bytes.len() as u64,
            ErrorCode::AssetUploadConflict,
        ),
    ] {
        let Err(error) = stage_object(
            body,
            temp.path().to_path_buf(),
            &expected_digest,
            expected_size,
        )
        .await
        else {
            panic!("invalid asset object unexpectedly staged");
        };
        assert_eq!(error.code(), expected_code);
        assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 0);
    }

    let failed = Body::from_stream(stream::once(async {
        Err::<Bytes, std::io::Error>(std::io::Error::other("stream failed"))
    }));
    let Err(error) = stage_object(
        failed,
        temp.path().to_path_buf(),
        &digest,
        bytes.len() as u64,
    )
    .await
    else {
        panic!("failed stream unexpectedly staged");
    };
    assert_eq!(error.code(), ErrorCode::AssetUploadIncomplete);
    let Err(error) =
        stage_object(Body::from(bytes), temp.path().join("missing"), &digest, 11).await
    else {
        panic!("missing staging directory unexpectedly accepted");
    };
    assert_eq!(error.code(), ErrorCode::DiskHardLimit);
}

#[tokio::test]
async fn upload_inventory_parsing_and_error_mapping_are_strict() {
    let digest = hex::encode([2; 32]);
    let valid = manifest(vec![AssetEntryV1 {
        path: "/asset.txt".to_owned(),
        sha256: digest.clone(),
        size: 5,
        content_type: "text/plain".to_owned(),
    }]);
    let objects = upload_inventory(&valid, [1; 32], 12, Some(([3; 32], 7))).unwrap();
    assert_eq!(objects.len(), 3);
    assert_eq!(
        objects
            .iter()
            .find(|object| object.sha256 == [2; 32])
            .unwrap()
            .kind,
        VersionObjectKind::AssetBlob
    );

    let conflicting = manifest(vec![
        AssetEntryV1 {
            path: "/a".to_owned(),
            sha256: digest.clone(),
            size: 5,
            content_type: "text/plain".to_owned(),
        },
        AssetEntryV1 {
            path: "/b".to_owned(),
            sha256: digest,
            size: 6,
            content_type: "text/plain".to_owned(),
        },
    ]);
    assert_eq!(
        upload_inventory(&conflicting, [1; 32], 12, None)
            .unwrap_err()
            .code(),
        ErrorCode::AssetUploadConflict
    );

    assert_eq!(parse_sha256(&hex::encode([4; 32])).unwrap(), [4; 32]);
    for invalid in [
        "0",
        "GG00000000000000000000000000000000000000000000000000000000000000",
        "AA00000000000000000000000000000000000000000000000000000000000000",
    ] {
        assert_eq!(
            parse_sha256(invalid).unwrap_err().code(),
            ErrorCode::AssetUploadConflict
        );
    }
    assert_eq!(
        parse_object_identity(&hex::encode([5; 32]), 9).unwrap(),
        ([5; 32], 9)
    );

    let account = AccountId::generate();
    let worker = WorkerId::generate();
    let upload = VersionUploadId::generate();
    assert_eq!(
        parse_upload_ids(
            &account.to_string(),
            &worker.to_string(),
            &upload.to_string()
        )
        .unwrap(),
        (account, worker, upload)
    );
    assert!(parse_upload_ids("bad", &worker.to_string(), &upload.to_string()).is_err());
    assert!(parse_upload_ids(&account.to_string(), &worker.to_string(), "bad").is_err());

    for (input, expected) in [
        (
            ErrorCode::ArtifactIntegrityError,
            ErrorCode::AssetIntegrityError,
        ),
        (ErrorCode::CacheEntryCorrupt, ErrorCode::AssetIntegrityError),
        (ErrorCode::LimitInvalid, ErrorCode::AssetLimitExceeded),
        (
            ErrorCode::ArtifactUnavailable,
            ErrorCode::AssetStorageUnavailable,
        ),
    ] {
        assert_eq!(
            map_asset_error(&PlatformError::new(input, "unsafe")).code(),
            expected
        );
    }
    assert_eq!(upload_incomplete().code(), ErrorCode::AssetUploadIncomplete);
    assert_eq!(upload_conflict().code(), ErrorCode::AssetUploadConflict);

    let request_id = RequestId::generate();
    let response = result_response_with_status(
        Ok(serde_json::json!({"ok": true})),
        StatusCode::CREATED,
        request_id,
    );
    assert_eq!(response.status(), StatusCode::CREATED);
    assert_eq!(
        to_bytes(response.into_body(), 1024).await.unwrap(),
        Bytes::from_static(br#"{"ok":true}"#)
    );
    assert_eq!(
        result_response_with_status(
            Err(PlatformError::new(
                ErrorCode::AssetUploadConflict,
                "conflict"
            )),
            StatusCode::CREATED,
            request_id,
        )
        .status(),
        StatusCode::CONFLICT
    );
}
