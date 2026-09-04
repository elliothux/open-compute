use super::*;

fn valid_input<'a>(objects: &'a [NewVersionUploadObject]) -> NewVersionUpload<'a> {
    NewVersionUpload {
        id: VersionUploadId::generate(),
        account_id: AccountId::generate(),
        worker_id: WorkerId::generate(),
        idempotency_key: "upload",
        input_fingerprint: [3; 32],
        content_kind: VersionContentKind::AssetsOnly,
        bundle: None,
        manifest_sha256: [1; 32],
        manifest_json: b"{}",
        routing_config_json: b"{}",
        objects,
        now_ms: 1,
        expires_at_ms: 2,
    }
}

#[test]
fn status_tokens_and_new_upload_shape_are_closed() {
    for (token, status) in [
        ("open", VersionUploadStatus::Open),
        ("finalizing", VersionUploadStatus::Finalizing),
        ("committed", VersionUploadStatus::Committed),
        ("aborted", VersionUploadStatus::Aborted),
        ("expired", VersionUploadStatus::Expired),
    ] {
        assert_eq!(VersionUploadStatus::parse(token).unwrap(), status);
        assert_eq!(status.as_str(), token);
    }
    assert_eq!(
        VersionUploadStatus::parse("legacy").unwrap_err().code(),
        ErrorCode::VersionInvariantViolation
    );

    let valid_objects = [NewVersionUploadObject {
        sha256: [1; 32],
        kind: VersionObjectKind::AssetManifest,
        size: 2,
    }];
    let valid = valid_input(&valid_objects);
    validate_new(&valid, 1, 2).unwrap();
    assert!(validate_new(&valid, 0, 2).is_err());
    assert!(validate_new(&valid, 2, 1).is_err());

    let mut invalid = valid.clone();
    invalid.content_kind = VersionContentKind::Worker;
    assert!(validate_new(&invalid, 1, 2).is_err());
    let mut invalid = valid.clone();
    invalid.idempotency_key = "";
    assert!(validate_new(&invalid, 1, 2).is_err());
    let long_key = "x".repeat(129);
    let mut invalid = valid.clone();
    invalid.idempotency_key = &long_key;
    assert!(validate_new(&invalid, 1, 2).is_err());
    let mut invalid = valid.clone();
    invalid.manifest_json = b"";
    assert!(validate_new(&invalid, 1, 2).is_err());
    let mut invalid = valid.clone();
    invalid.routing_config_json = b"";
    assert!(validate_new(&invalid, 1, 2).is_err());
    let mut invalid = valid.clone();
    invalid.expires_at_ms = invalid.now_ms;
    assert!(validate_new(&invalid, 1, 2).is_err());
    let mut invalid = valid.clone();
    invalid.objects = &[];
    assert!(validate_new(&invalid, 1, 2).is_err());

    let wrong_manifest = [NewVersionUploadObject {
        sha256: [2; 32],
        kind: VersionObjectKind::AssetBlob,
        size: 2,
    }];
    assert!(validate_new(&valid_input(&wrong_manifest), 1, 2).is_err());
    let duplicate = [
        NewVersionUploadObject {
            sha256: [1; 32],
            kind: VersionObjectKind::AssetManifest,
            size: 2,
        },
        NewVersionUploadObject {
            sha256: [1; 32],
            kind: VersionObjectKind::AssetBlob,
            size: 2,
        },
    ];
    assert!(validate_new(&valid_input(&duplicate), 1, 2).is_err());

    let missing_bundle_object = [
        NewVersionUploadObject {
            sha256: [1; 32],
            kind: VersionObjectKind::AssetManifest,
            size: 2,
        },
        NewVersionUploadObject {
            sha256: [2; 32],
            kind: VersionObjectKind::AssetBlob,
            size: 4,
        },
    ];
    let mut worker = valid_input(&missing_bundle_object);
    worker.content_kind = VersionContentKind::Worker;
    worker.bundle = Some(([2; 32], 4));
    assert!(validate_new(&worker, 1, 2).is_err());

    let worker_objects = [
        NewVersionUploadObject {
            sha256: [1; 32],
            kind: VersionObjectKind::AssetManifest,
            size: 2,
        },
        NewVersionUploadObject {
            sha256: [2; 32],
            kind: VersionObjectKind::Bundle,
            size: 4,
        },
    ];
    let mut worker = valid_input(&worker_objects);
    worker.content_kind = VersionContentKind::Worker;
    worker.bundle = Some(([2; 32], 4));
    assert!(validate_new(&worker, 1, 2).is_ok());
}
