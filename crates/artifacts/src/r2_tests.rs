use super::*;
use open_compute_core::PlatformId;

fn locator() -> R2BucketLocator {
    let resource_id = ResourceId::generate();
    let physical_prefix = format!("tenant/r2/v1/{resource_id}/");
    R2BucketLocator {
        resource_id,
        object_prefix: format!("{physical_prefix}objects/"),
        physical_prefix,
    }
}

#[test]
fn user_keys_are_not_normalized_and_respect_dynamic_budget() {
    let locator = locator();
    for key in ["", "/", "a//b", "%2F", "中文", "+", " a "] {
        assert_eq!(UserObjectKey::parse(key, &locator).unwrap().as_str(), key);
    }
    for key in [".", "..", "a/./b", "a/../b"] {
        assert_eq!(
            UserObjectKey::parse(key, &locator).unwrap_err().code(),
            ErrorCode::R2KeyInvalid
        );
    }
    let exact = "x".repeat(locator.max_user_key_bytes());
    assert!(UserObjectKey::parse(&exact, &locator).is_ok());
    assert_eq!(
        UserObjectKey::parse(&(exact + "x"), &locator)
            .unwrap_err()
            .code(),
        ErrorCode::R2KeyTooLarge
    );
}

#[test]
fn range_condition_etag_and_metadata_are_strict() {
    assert_eq!(
        R2Range {
            offset: Some(3),
            length: Some(4),
            suffix: None,
        }
        .header()
        .unwrap(),
        "bytes=3-6"
    );
    assert_eq!(
        R2Range {
            offset: None,
            length: None,
            suffix: Some(5),
        }
        .header()
        .unwrap(),
        "bytes=-5"
    );
    assert!(
        R2Range {
            offset: Some(0),
            length: Some(0),
            suffix: None,
        }
        .header()
        .is_err()
    );
    assert_eq!(unquote_etag("\"abc\"").unwrap(), "abc");
    assert_eq!(quote_etag("abc").unwrap(), "\"abc\"");
    assert!(quote_etag("a\"b").is_err());

    let mut custom = BTreeMap::new();
    custom.insert("b".to_owned(), "2".to_owned());
    custom.insert("a".to_owned(), "1".to_owned());
    assert_eq!(
        canonical_custom_metadata(&custom).unwrap(),
        br#"{"a":"1","b":"2"}"#
    );
    custom.insert("large".to_owned(), "x".repeat(4096));
    assert_eq!(
        canonical_custom_metadata(&custom).unwrap_err().code(),
        ErrorCode::R2MetadataTooLarge
    );
}

#[test]
fn content_range_and_md5_file_are_exact() {
    assert_eq!(
        parse_content_range("bytes 2-5/9"),
        Some((
            R2Range {
                offset: Some(2),
                length: Some(4),
                suffix: None,
            },
            9,
        ))
    );
    assert!(parse_content_range("bytes */9").is_none());
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("value");
    std::fs::write(&path, b"hello").unwrap();
    assert_eq!(
        hex::encode(md5_file(&path, 5).unwrap()),
        "5d41402abc4b2a76b9719d911017c592"
    );
    assert!(md5_file(&path, 4).is_err());
}

#[test]
fn conditions_use_exact_etag_and_upload_time() {
    let meta = R2ObjectMetadata {
        key: "k".to_owned(),
        version: uuid::Uuid::now_v7().to_string(),
        size: 1,
        etag: "etag".to_owned(),
        http_etag: "\"etag\"".to_owned(),
        uploaded: 100,
        http_metadata: R2HttpMetadata::default(),
        custom_metadata: BTreeMap::new(),
        range: None,
        md5: "00".repeat(16),
        storage_class: "Standard".to_owned(),
    };
    assert!(condition_matches(
        &R2Condition {
            etag_matches: vec!["\"etag\"".to_owned()],
            uploaded_before: Some(100),
            uploaded_after: Some(99),
            ..R2Condition::default()
        },
        &meta
    ));
    assert!(!condition_matches(
        &R2Condition {
            etag_does_not_match: vec!["etag".to_owned()],
            ..R2Condition::default()
        },
        &meta
    ));
}

#[tokio::test]
async fn typed_store_rejects_local_invalid_inputs_and_identity_collisions() {
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
    assert_eq!(
        store.locator(resource_id, "wrong").unwrap_err().code(),
        ErrorCode::ResourceInvariantViolation
    );
    let locator = store
        .locator(resource_id, &store.physical_prefix(resource_id))
        .unwrap();
    assert_eq!(
        store
            .ensure_identity(
                &locator,
                &R2BucketIdentity {
                    schema_version: 2,
                    platform_id: PlatformId::generate(),
                    resource_id,
                    created_at_ms: 1,
                },
            )
            .await
            .unwrap_err()
            .code(),
        ErrorCode::ResourceInvariantViolation
    );
    let identity = R2BucketIdentity {
        schema_version: 1,
        platform_id: PlatformId::generate(),
        resource_id,
        created_at_ms: 1,
    };
    store.ensure_identity(&locator, &identity).await.unwrap();
    let conflicting = R2BucketIdentity {
        platform_id: PlatformId::generate(),
        ..identity
    };
    assert_eq!(
        store
            .ensure_identity(&locator, &conflicting)
            .await
            .unwrap_err()
            .code(),
        ErrorCode::R2PrefixCollision
    );

    let key = UserObjectKey::parse("key", &locator).unwrap();
    let missing = R2UploadSource {
        path: std::path::PathBuf::from("/definitely/missing/open-compute-r2"),
        length: 0,
        md5: Md5::digest([]).into(),
        version: uuid::Uuid::now_v7().to_string(),
    };
    assert_eq!(
        store
            .put_file(&locator, &key, &missing, &R2PutOptions::default())
            .await
            .unwrap_err()
            .code(),
        ErrorCode::R2ProviderUnavailable
    );
    assert_eq!(
        md5_file(&missing.path, 0).unwrap_err().code(),
        ErrorCode::R2ProviderUnavailable
    );
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("body");
    std::fs::write(&path, b"body").unwrap();
    let valid = R2UploadSource {
        path: path.clone(),
        length: 4,
        md5: Md5::digest(b"body").into(),
        version: uuid::Uuid::now_v7().to_string(),
    };
    let wrong_length = R2UploadSource {
        length: 3,
        ..R2UploadSource {
            path: path.clone(),
            length: valid.length,
            md5: valid.md5,
            version: valid.version.clone(),
        }
    };
    assert_eq!(
        store
            .put_file(&locator, &key, &wrong_length, &R2PutOptions::default())
            .await
            .unwrap_err()
            .code(),
        ErrorCode::R2ObjectMetadataInvalid
    );
    let bad_version = R2UploadSource {
        path: path.clone(),
        length: valid.length,
        md5: valid.md5,
        version: "bad".to_owned(),
    };
    assert_eq!(
        store
            .put_file(&locator, &key, &bad_version, &R2PutOptions::default())
            .await
            .unwrap_err()
            .code(),
        ErrorCode::ResourceInvariantViolation
    );
    assert_eq!(
        store
            .put_file(
                &locator,
                &key,
                &valid,
                &R2PutOptions {
                    expected_md5: Some([0; 16]),
                    ..R2PutOptions::default()
                },
            )
            .await
            .unwrap_err()
            .code(),
        ErrorCode::R2PreconditionFailed
    );
    for condition in [
        R2Condition {
            uploaded_before: Some(1),
            ..R2Condition::default()
        },
        R2Condition {
            etag_matches: vec!["a".to_owned(), "b".to_owned()],
            ..R2Condition::default()
        },
    ] {
        assert!(
            store
                .put_file(
                    &locator,
                    &key,
                    &valid,
                    &R2PutOptions {
                        only_if: Some(condition),
                        ..R2PutOptions::default()
                    },
                )
                .await
                .is_err()
        );
    }
    assert_eq!(
        store.delete(&locator, &[]).await.unwrap_err().code(),
        ErrorCode::R2InvalidOptions
    );
    assert_eq!(
        store
            .delete(&locator, &vec![key.clone(); R2_MAX_DELETE_KEYS + 1])
            .await
            .unwrap_err()
            .code(),
        ErrorCode::R2InvalidOptions
    );
    assert!(store.list(&locator, "", None, 0, None).await.is_err());
    assert!(
        store
            .list(&locator, "", Some(""), R2_MAX_LIST_LIMIT, None)
            .await
            .is_err()
    );
    assert_eq!(
        store
            .get(
                &locator,
                &key,
                Some(R2Range {
                    offset: Some(0),
                    length: Some(0),
                    suffix: None,
                }),
                None,
            )
            .await
            .unwrap_err()
            .code(),
        ErrorCode::R2InvalidOptions
    );

    for value in ["", "\"\"", "bad\n"] {
        assert_eq!(
            unquote_etag(value).unwrap_err().code(),
            ErrorCode::R2ObjectMetadataInvalid
        );
    }
    for value in ["bad", "bytes */9", "bytes 4-2/9", "bytes a-b/9"] {
        assert!(parse_content_range(value).is_none());
    }
    assert!(http_date_millis("Wed, 21 Oct 2015 07:28:00 GMT").is_some());
    assert!(http_date_millis("bad").is_none());
    assert!(millis_datetime(-1).to_millis().is_ok());
}

#[tokio::test]
async fn typed_store_round_trips_identity_object_range_list_and_delete() {
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
    let store = R2ObjectStore::new(client);
    let resource_id = ResourceId::generate();
    let locator = store
        .locator(resource_id, &store.physical_prefix(resource_id))
        .unwrap();
    let identity = R2BucketIdentity {
        schema_version: 1,
        platform_id: PlatformId::generate(),
        resource_id,
        created_at_ms: 10,
    };
    store.ensure_identity(&locator, &identity).await.unwrap();
    assert_eq!(store.read_identity(&locator).await.unwrap(), Some(identity));

    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("upload");
    std::fs::write(&path, b"hello world").unwrap();
    let source = R2UploadSource {
        path,
        length: 11,
        md5: Md5::digest(b"hello world").into(),
        version: uuid::Uuid::now_v7().to_string(),
    };
    let key = UserObjectKey::parse("folder/a + %.txt", &locator).unwrap();
    let mut custom = BTreeMap::new();
    custom.insert("作者".to_owned(), "Elliot".to_owned());
    let options = R2PutOptions {
        http_metadata: R2HttpMetadata {
            content_type: Some("text/plain; charset=utf-8".to_owned()),
            cache_control: Some("max-age=60".to_owned()),
            ..R2HttpMetadata::default()
        },
        custom_metadata: custom.clone(),
        expected_md5: Some(source.md5),
        ..R2PutOptions::default()
    };
    let put = store
        .put_file(&locator, &key, &source, &options)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(put.key, key.as_str());
    assert_eq!(put.custom_metadata, custom);
    assert_eq!(
        put.http_metadata.content_type,
        options.http_metadata.content_type
    );
    assert_eq!(put.size, 11);
    assert_eq!(store.head(&locator, &key).await.unwrap(), Some(put.clone()));

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
        )
        .await
        .unwrap()
    else {
        panic!("expected body")
    };
    assert_eq!(
        download.body.collect().await.unwrap().into_bytes(),
        &b"world"[..]
    );
    assert_eq!(download.metadata.range.unwrap().length, Some(5));

    let page = store
        .list(&locator, "folder/", Some("/"), 1000, None)
        .await
        .unwrap();
    assert_eq!(page.objects.len(), 1);
    assert_eq!(page.objects[0].key, key.as_str());
    assert!(page.provider_token.is_none());

    let failed = R2Condition {
        etag_matches: vec!["different".to_owned()],
        ..R2Condition::default()
    };
    assert!(matches!(
        store
            .get(&locator, &key, None, Some(&failed))
            .await
            .unwrap(),
        R2GetResult::Precondition(_)
    ));
    store
        .delete(&locator, std::slice::from_ref(&key))
        .await
        .unwrap();
    assert!(store.head(&locator, &key).await.unwrap().is_none());
    assert!(store.is_empty(&locator).await.unwrap());
    store.delete_identity(&locator).await.unwrap();
    assert!(store.read_identity(&locator).await.unwrap().is_none());
}

#[tokio::test]
async fn identical_user_keys_remain_isolated_between_logical_buckets() {
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
    let store =
        R2ObjectStore::new(S3ArtifactClient::connect(&config, &credentials, 1024 * 1024).unwrap());
    let first_id = ResourceId::generate();
    let second_id = ResourceId::generate();
    let first = store
        .locator(first_id, &store.physical_prefix(first_id))
        .unwrap();
    let second = store
        .locator(second_id, &store.physical_prefix(second_id))
        .unwrap();
    let key = "same/key";
    let temp = tempfile::tempdir().unwrap();

    for (locator, body) in [
        (&first, b"first".as_slice()),
        (&second, b"second".as_slice()),
    ] {
        let path = temp.path().join(locator.resource_id.to_string());
        std::fs::write(&path, body).unwrap();
        store
            .put_file(
                locator,
                &UserObjectKey::parse(key, locator).unwrap(),
                &R2UploadSource {
                    path,
                    length: body.len() as u64,
                    md5: Md5::digest(body).into(),
                    version: uuid::Uuid::now_v7().to_string(),
                },
                &R2PutOptions::default(),
            )
            .await
            .unwrap()
            .unwrap();
    }

    for (locator, expected) in [
        (&first, b"first".as_slice()),
        (&second, b"second".as_slice()),
    ] {
        let R2GetResult::Body(object) = store
            .get(
                locator,
                &UserObjectKey::parse(key, locator).unwrap(),
                None,
                None,
            )
            .await
            .unwrap()
        else {
            panic!("expected isolated object body")
        };
        assert_eq!(object.body.collect().await.unwrap().into_bytes(), expected);
    }
}

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
