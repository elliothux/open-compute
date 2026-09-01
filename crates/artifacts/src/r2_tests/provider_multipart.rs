use super::*;

#[tokio::test]
async fn r2_provider_preflight_verifies_required_capabilities_and_cleans_up() {
    let mock = crate::MockS3::spawn("bucket").await;
    let config = open_compute_core::S3Config {
        endpoint: mock.endpoint.clone(),
        bucket: "bucket".to_owned(),
        ..open_compute_core::S3Config::default()
    };
    let env = crate::MapEnv::new()
        .with("S3_ACCESS_KEY_ID", "test-access")
        .with("S3_SECRET_ACCESS_KEY", "test-secret");
    let credentials = crate::resolve_s3_credentials_with(&config, &env).unwrap();
    let client = S3ArtifactClient::connect(&config, &credentials, 1024 * 1024).unwrap();
    let outcome = crate::preflight_r2(
        &client,
        PlatformId::generate(),
        open_compute_core::StartupId::generate(),
    )
    .await
    .unwrap();
    assert_eq!(outcome.objects, 3);
    assert!(outcome.multi_delete);
    assert!(mock.keys().is_empty());
}

#[tokio::test]
async fn typed_store_round_trips_ssec_storage_class_and_multipart() {
    let mock = crate::MockS3::spawn("bucket").await;
    let config = open_compute_core::S3Config {
        endpoint: mock.endpoint.clone(),
        bucket: "bucket".to_owned(),
        ..open_compute_core::S3Config::default()
    };
    let credentials = crate::resolve_s3_credentials_with(
        &config,
        &crate::MapEnv::new()
            .with("S3_ACCESS_KEY_ID", "test-access")
            .with("S3_SECRET_ACCESS_KEY", "test-secret"),
    )
    .unwrap();
    let store =
        R2ObjectStore::new(S3ArtifactClient::connect(&config, &credentials, 1024 * 1024).unwrap());
    let resource_id = ResourceId::generate();
    let locator = store
        .locator(resource_id, &store.physical_prefix(resource_id))
        .unwrap();
    store
        .ensure_identity(
            &locator,
            &R2BucketIdentity {
                schema_version: 1,
                platform_id: PlatformId::generate(),
                resource_id,
                created_at_ms: 1,
            },
        )
        .await
        .unwrap();
    let temp = tempfile::tempdir().unwrap();
    let ia_path = temp.path().join("ia");
    std::fs::write(&ia_path, b"ia").unwrap();
    let ia_key = UserObjectKey::parse("a.bin").unwrap();
    let ia = store
        .put_file(
            &locator,
            &ia_key,
            &upload_source(ia_path, b"ia"),
            &R2PutOptions {
                storage_class: R2StorageClass::InfrequentAccess,
                checksum: Some(R2ChecksumAlgorithm::Sha256(hash_bytes(b"ia").sha256)),
                ..R2PutOptions::default()
            },
            None,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(ia.storage_class, "InfrequentAccess");

    let ssec = R2SsecKey::parse_hex(&"ab".repeat(32)).unwrap();
    let secret_path = temp.path().join("secret");
    std::fs::write(&secret_path, b"secret").unwrap();
    let secret_key = UserObjectKey::parse("z.bin").unwrap();
    let put = store
        .put_file(
            &locator,
            &secret_key,
            &upload_source(secret_path, b"secret"),
            &R2PutOptions {
                ssec: Some(ssec.clone()),
                ..R2PutOptions::default()
            },
            None,
        )
        .await
        .unwrap()
        .unwrap();
    assert!(put.ssec_key_md5.is_some());
    assert_eq!(
        store
            .head(&locator, &secret_key, Some(&ssec))
            .await
            .unwrap()
            .unwrap()
            .ssec_key_md5,
        put.ssec_key_md5
    );
    assert_eq!(
        store
            .get(&locator, &secret_key, None, None, None)
            .await
            .unwrap_err()
            .code(),
        ErrorCode::R2SsecInvalid
    );
    let R2GetResult::Body(download) = store
        .get(&locator, &secret_key, None, None, Some(&ssec))
        .await
        .unwrap()
    else {
        panic!("expected sse-c body")
    };
    assert_eq!(
        download.body.collect().await.unwrap().into_bytes(),
        &b"secret"[..]
    );
    let mpu_key = UserObjectKey::parse("mpu.bin").unwrap();
    let version = uuid::Uuid::now_v7().hyphenated().to_string();
    let upload_id = store
        .create_multipart_upload(
            &locator,
            &mpu_key,
            &version,
            &R2MultipartCreateOptions::default(),
        )
        .await
        .unwrap();
    assert_eq!(
        store
            .list_multipart_upload_ids(&locator, &mpu_key)
            .await
            .unwrap(),
        vec![upload_id.clone()]
    );
    let part_path = temp.path().join("part");
    std::fs::write(&part_path, b"part-body").unwrap();
    let part = store
        .upload_part(
            &locator,
            &mpu_key,
            &upload_id,
            1,
            &upload_source(part_path, b"part-body"),
            None,
        )
        .await
        .unwrap();
    let completed = store
        .complete_multipart_upload(&locator, &mpu_key, &upload_id, &[part], None)
        .await
        .unwrap();
    assert_eq!(completed.key, "mpu.bin");
    assert_eq!(completed.version, version);
    assert_eq!(completed.checksums, R2Checksums::default());
    assert!(
        store
            .list_multipart_upload_ids(&locator, &mpu_key)
            .await
            .unwrap()
            .is_empty()
    );
    let abort_id = store
        .create_multipart_upload(
            &locator,
            &mpu_key,
            &uuid::Uuid::now_v7().hyphenated().to_string(),
            &R2MultipartCreateOptions::default(),
        )
        .await
        .unwrap();
    store
        .abort_multipart_upload(&locator, &mpu_key, &abort_id)
        .await
        .unwrap();
}
