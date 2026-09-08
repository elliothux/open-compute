use super::*;

#[tokio::test]
async fn local_typed_r2_round_trips_metadata_conditions_ssec_and_multipart() {
    let fixture = Fixture::new();
    let store = R2ObjectStore::new(fixture.backend.clone());
    let resource_id = ResourceId::generate();
    let locator = store
        .locator(resource_id, &store.physical_prefix(resource_id))
        .unwrap();
    store
        .ensure_identity(
            &locator,
            &R2BucketIdentity {
                schema_version: 1,
                platform_id: fixture.platform_id,
                resource_id,
                created_at_ms: 1,
            },
        )
        .await
        .unwrap();

    let body = b"local typed R2 body";
    let source_path = fixture.config.path.parent().unwrap().join("r2-source");
    write_private(&source_path, body);
    let source = R2UploadSource {
        path: source_path,
        length: body.len() as u64,
        checksums: hash_bytes(body),
        version: uuid::Uuid::now_v7().hyphenated().to_string(),
    };
    let key = UserObjectKey::parse("folder/中文 + %.txt").unwrap();
    let ssec = R2SsecKey::parse_hex(&"ab".repeat(32)).unwrap();
    let put = store
        .put_file(
            &locator,
            &key,
            &source,
            &R2PutOptions {
                http_metadata: R2HttpMetadata {
                    content_type: Some("text/plain; charset=utf-8".to_owned()),
                    ..R2HttpMetadata::default()
                },
                custom_metadata: BTreeMap::from([("author".to_owned(), "local".to_owned())]),
                checksum: Some(R2ChecksumAlgorithm::Sha256(source.checksums.sha256)),
                storage_class: R2StorageClass::InfrequentAccess,
                ssec: Some(ssec.clone()),
                ..R2PutOptions::default()
            },
            None,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(put.etag, hex::encode(source.checksums.md5));
    assert_eq!(put.storage_class, "InfrequentAccess");
    assert_eq!(put.ssec_key_md5.as_deref(), Some(ssec.md5_hex().as_str()));
    let R2GetResult::Body(download) = store
        .get(
            &locator,
            &key,
            Some(R2Range {
                offset: Some(6),
                length: Some(5),
                suffix: None,
            }),
            None,
            Some(&ssec),
        )
        .await
        .unwrap()
    else {
        panic!("expected local R2 body")
    };
    assert_eq!(
        download.body.collect().await.unwrap().into_bytes(),
        &body[6..11]
    );
    assert!(store.get(&locator, &key, None, None, None).await.is_err());

    let multipart_key = UserObjectKey::parse("multipart").unwrap();
    let version = uuid::Uuid::now_v7().hyphenated().to_string();
    let upload_id = store
        .create_multipart_upload(
            &locator,
            &multipart_key,
            &version,
            &R2MultipartCreateOptions::default(),
        )
        .await
        .unwrap();
    let part_path = fixture.config.path.parent().unwrap().join("r2-part");
    write_private(&part_path, b"multipart-local");
    let part_source = crate::R2PartSource {
        path: part_path,
        length: 15,
    };
    let part = store
        .upload_part(&locator, &multipart_key, &upload_id, 1, &part_source, None)
        .await
        .unwrap();
    assert_eq!(part.etag, hex::encode(md5::Md5::digest(b"multipart-local")));
    let part_digest = hex::decode(&part.etag).unwrap();
    let expected_complete_etag = format!("{}-1", hex::encode(md5::Md5::digest(part_digest)));
    let completed = store
        .complete_multipart_upload(&locator, &multipart_key, &upload_id, &[part], None)
        .await
        .unwrap();
    assert_eq!(completed.version, version);
    assert_eq!(completed.etag, expected_complete_etag);
    assert!(
        store
            .list_multipart_upload_ids(&locator, &multipart_key)
            .await
            .unwrap()
            .is_empty()
    );
}
