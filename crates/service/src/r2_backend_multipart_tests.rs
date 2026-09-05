use super::*;

fn record() -> R2MultipartUploadRecord {
    R2MultipartUploadRecord {
        upload_id: "upload".to_owned(),
        resource_id: ResourceId::generate(),
        account_id: AccountId::generate(),
        object_key: "object".to_owned(),
        provider_upload_id: Some("provider".to_owned()),
        storage_class: "Standard".to_owned(),
        http_metadata: serde_json::to_string(&R2HttpMetadata::default()).unwrap(),
        custom_metadata: serde_json::to_string(&BTreeMap::from([(
            "key".to_owned(),
            "value".to_owned(),
        )]))
        .unwrap(),
        ssec_key_md5: None,
        ssec_envelope: None,
        object_version: "version".to_owned(),
        completion_manifest: None,
        completed_metadata: None,
        state: R2MultipartState::Completed,
    }
}

fn metadata() -> R2ObjectMetadata {
    R2ObjectMetadata {
        key: "object".to_owned(),
        version: "version".to_owned(),
        size: 7,
        etag: "etag".to_owned(),
        http_etag: "\"etag\"".to_owned(),
        uploaded: 1,
        http_metadata: Some(R2HttpMetadata::default()),
        custom_metadata: Some(BTreeMap::from([("key".to_owned(), "value".to_owned())])),
        range: None,
        checksums: open_compute_artifacts::R2Checksums::default(),
        storage_class: "Standard".to_owned(),
        ssec_key_md5: None,
    }
}

#[test]
fn persisted_completion_is_exact_canonical_and_replay_safe() {
    let parts = vec![open_compute_artifacts::R2UploadedPart {
        part_number: 1,
        etag: "etag".to_owned(),
    }];
    let mut row = record();
    assert!(completion_parts(&row, &parts).is_err());
    row.completion_manifest = Some("not-json".to_owned());
    assert!(completion_parts(&row, &parts).is_err());
    row.completion_manifest = Some("[]".to_owned());
    assert_eq!(
        completion_parts(&row, &[]).unwrap_err().code(),
        ErrorCode::R2MultipartInvalid
    );
    row.completion_manifest = Some(format!(" {} ", canonical_completion(&parts).unwrap()));
    assert!(completion_parts(&row, &parts).is_err());
    row.completion_manifest = Some(canonical_completion(&parts).unwrap());
    assert_eq!(completion_parts(&row, &parts).unwrap(), parts);
    assert!(
        completion_parts(
            &row,
            &[open_compute_artifacts::R2UploadedPart {
                part_number: 1,
                etag: "different".to_owned(),
            }],
        )
        .is_err()
    );

    assert!(completed_metadata(&row).is_err());
    row.completed_metadata = Some("not-json".to_owned());
    assert!(completed_metadata(&row).is_err());
    row.completed_metadata = Some(serde_json::to_string(&metadata()).unwrap());
    assert_eq!(completed_metadata(&row).unwrap(), metadata());
    for invalid in [
        R2ObjectMetadata {
            key: "other".to_owned(),
            ..metadata()
        },
        R2ObjectMetadata {
            version: "other".to_owned(),
            ..metadata()
        },
    ] {
        row.completed_metadata = Some(serde_json::to_string(&invalid).unwrap());
        assert_eq!(
            completed_metadata(&row).unwrap_err().code(),
            ErrorCode::R2ObjectMetadataInvalid
        );
    }
}

#[test]
fn completed_object_metadata_must_match_every_durable_input() {
    let row = record();
    let requested = [open_compute_artifacts::R2UploadedPart {
        part_number: 1,
        etag: "etag".to_owned(),
    }];
    let stored = [R2MultipartPartRecord {
        part_number: 1,
        etag: "etag".to_owned(),
        size: 7,
    }];
    validate_completed_object(&row, &requested, &stored, &metadata()).unwrap();
    assert!(validate_completed_object(&row, &requested, &[], &metadata()).is_err());

    let mut variants = Vec::new();
    let mut value = metadata();
    value.key = "other".to_owned();
    variants.push(value);
    let mut value = metadata();
    value.version = "other".to_owned();
    variants.push(value);
    let mut value = metadata();
    value.size = 8;
    variants.push(value);
    let mut value = metadata();
    value.range = Some(open_compute_artifacts::R2Range {
        offset: Some(0),
        length: Some(1),
        suffix: None,
    });
    variants.push(value);
    let mut value = metadata();
    value.storage_class = "InfrequentAccess".to_owned();
    variants.push(value);
    let mut value = metadata();
    value.ssec_key_md5 = Some("digest".to_owned());
    variants.push(value);
    let mut value = metadata();
    value.http_metadata = None;
    variants.push(value);
    let mut value = metadata();
    value.custom_metadata = None;
    variants.push(value);
    let mut value = metadata();
    value.etag.clear();
    variants.push(value);
    let mut value = metadata();
    value.etag = "bad\nvalue".to_owned();
    variants.push(value);
    let mut value = metadata();
    value.http_etag = "etag".to_owned();
    variants.push(value);
    for value in variants {
        assert_eq!(
            validate_completed_object(&row, &requested, &stored, &value)
                .unwrap_err()
                .code(),
            ErrorCode::R2ObjectMetadataInvalid
        );
    }

    let overflow_parts = [
        open_compute_artifacts::R2UploadedPart {
            part_number: 1,
            etag: "one".to_owned(),
        },
        open_compute_artifacts::R2UploadedPart {
            part_number: 2,
            etag: "two".to_owned(),
        },
    ];
    let overflow_stored = [
        R2MultipartPartRecord {
            part_number: 1,
            etag: "one".to_owned(),
            size: u64::MAX,
        },
        R2MultipartPartRecord {
            part_number: 2,
            etag: "two".to_owned(),
            size: 1,
        },
    ];
    assert_eq!(
        validate_completed_object(&row, &overflow_parts, &overflow_stored, &metadata())
            .unwrap_err()
            .code(),
        ErrorCode::R2ObjectMetadataInvalid
    );
}

#[test]
fn missing_ssec_envelope_and_digest_are_rejected_together() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join("data");
    let storage = PlatformStorage::bootstrap(
        &open_compute_core::DataConfig {
            path: root.clone(),
            master_key_file: root.join("keys/master.key"),
            master_key_env: None,
            sqlite_busy_timeout_ms: 5_000,
            free_space_soft_bytes: 1_073_741_824,
            free_space_hard_bytes: 268_435_456,
        },
        &open_compute_core::SystemClock,
    )
    .unwrap();
    let row = record();
    assert!(open_ssec(&storage, &row).unwrap().is_none());
    let mut incomplete = row.clone();
    incomplete.ssec_key_md5 = Some("digest".to_owned());
    assert!(open_ssec(&storage, &incomplete).is_err());
    incomplete.ssec_envelope = Some("not-json".to_owned());
    assert!(open_ssec(&storage, &incomplete).is_err());
    incomplete.ssec_key_md5 = None;
    assert!(open_ssec(&storage, &incomplete).is_err());
}
