use super::*;
use crate::r2_codec::{
    canonical_custom_metadata, http_date_millis, millis_datetime, quote_etag, unquote_etag,
};
use crate::r2_model::{
    R2Checksums, R2EtagMatch, R2MultipartCreateOptions, R2SsecKey, R2StorageClass,
};
use open_compute_core::PlatformId;
use std::collections::BTreeMap;

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
fn user_keys_are_not_normalized_and_use_cloudflare_limit() {
    let locator = locator();
    assert_eq!(locator.resource_id(), locator.resource_id);
    for key in [
        "", "/", ".", "..", "a//b", "a/./b", "a/../b", "%2F", "中文", "+", " a ",
    ] {
        assert_eq!(UserObjectKey::parse(key).unwrap().as_str(), key);
    }
    let exact = "x".repeat(crate::R2_MAX_KEY_BYTES);
    assert!(UserObjectKey::parse(&exact).is_ok());
    assert_eq!(
        UserObjectKey::parse(&(exact + "x")).unwrap_err().code(),
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
            offset: Some(3),
            length: None,
            suffix: None,
        }
        .header()
        .unwrap(),
        "bytes=3-"
    );
    assert_eq!(
        R2Range {
            offset: None,
            length: Some(4),
            suffix: None,
        }
        .header()
        .unwrap(),
        "bytes=0-3"
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
fn exposed_checksums_match_cloudflare_md5_plus_requested_algorithm_semantics() {
    let computed = hash_bytes(b"x");
    assert_eq!(
        computed.exposed(None),
        R2Checksums {
            md5: Some(hex::encode(computed.md5)),
            ..R2Checksums::default()
        }
    );
    for (requested, expected) in [
        (
            R2ChecksumAlgorithm::Md5(computed.md5),
            R2Checksums {
                md5: Some(hex::encode(computed.md5)),
                ..R2Checksums::default()
            },
        ),
        (
            R2ChecksumAlgorithm::Sha1(computed.sha1),
            R2Checksums {
                md5: Some(hex::encode(computed.md5)),
                sha1: Some(hex::encode(computed.sha1)),
                ..R2Checksums::default()
            },
        ),
        (
            R2ChecksumAlgorithm::Sha256(computed.sha256),
            R2Checksums {
                md5: Some(hex::encode(computed.md5)),
                sha256: Some(hex::encode(computed.sha256)),
                ..R2Checksums::default()
            },
        ),
        (
            R2ChecksumAlgorithm::Sha384(computed.sha384),
            R2Checksums {
                md5: Some(hex::encode(computed.md5)),
                sha384: Some(hex::encode(computed.sha384)),
                ..R2Checksums::default()
            },
        ),
        (
            R2ChecksumAlgorithm::Sha512(computed.sha512),
            R2Checksums {
                md5: Some(hex::encode(computed.md5)),
                sha512: Some(hex::encode(computed.sha512)),
                ..R2Checksums::default()
            },
        ),
    ] {
        assert_eq!(computed.exposed(Some(&requested)), expected);
    }
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
        http_metadata: Some(R2HttpMetadata::default()),
        custom_metadata: Some(BTreeMap::new()),
        range: None,
        checksums: R2Checksums {
            md5: Some("00".repeat(16)),
            ..R2Checksums::default()
        },
        storage_class: "Standard".to_owned(),
        ssec_key_md5: None,
    };
    assert!(
        R2Condition {
            etag_matches: vec![R2EtagMatch::Strong {
                value: "etag".to_owned()
            }],
            uploaded_before: Some(100),
            uploaded_after: Some(99),
            ..R2Condition::default()
        }
        .matches_object(&meta.etag, meta.uploaded)
    );
    assert!(
        !R2Condition {
            etag_does_not_match: vec![R2EtagMatch::Strong {
                value: "etag".to_owned()
            }],
            ..R2Condition::default()
        }
        .matches_object(&meta.etag, meta.uploaded)
    );
    assert!(
        !R2Condition {
            etag_matches: vec![R2EtagMatch::Weak {
                value: "etag".to_owned()
            }],
            http_headers: true,
            ..R2Condition::default()
        }
        .matches_object(&meta.etag, meta.uploaded)
    );
    assert!(
        !R2Condition {
            etag_does_not_match: vec![R2EtagMatch::Weak {
                value: "etag".to_owned()
            }],
            http_headers: true,
            ..R2Condition::default()
        }
        .matches_object(&meta.etag, meta.uploaded)
    );
    assert!(
        R2Condition {
            etag_matches: vec![R2EtagMatch::Strong {
                value: "etag".to_owned()
            }],
            uploaded_before: Some(0),
            http_headers: true,
            ..R2Condition::default()
        }
        .matches_object(&meta.etag, meta.uploaded)
    );
    let header_none_match = R2Condition {
        etag_does_not_match: vec![R2EtagMatch::Strong {
            value: "different".to_owned(),
        }],
        uploaded_after: Some(1_000),
        http_headers: true,
        ..R2Condition::default()
    };
    assert!(header_none_match.matches_object(&meta.etag, meta.uploaded));
    assert!(header_none_match.matches_missing());
}

fn upload_source(path: std::path::PathBuf, bytes: &[u8]) -> R2UploadSource {
    R2UploadSource {
        path,
        length: u64::try_from(bytes.len()).unwrap(),
        checksums: hash_bytes(bytes),
        version: uuid::Uuid::now_v7().to_string(),
    }
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

    let key = UserObjectKey::parse("key").unwrap();
    let missing = R2UploadSource {
        path: std::path::PathBuf::from("/definitely/missing/open-compute-r2"),
        length: 0,
        checksums: hash_bytes(b""),
        version: uuid::Uuid::now_v7().to_string(),
    };
    assert_eq!(
        store
            .put_file(&locator, &key, &missing, &R2PutOptions::default(), None)
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
    let valid = upload_source(path.clone(), b"body");
    let wrong_length = R2UploadSource {
        length: 3,
        ..R2UploadSource {
            path: path.clone(),
            length: valid.length,
            checksums: valid.checksums.clone(),
            version: valid.version.clone(),
        }
    };
    assert_eq!(
        store
            .put_file(
                &locator,
                &key,
                &wrong_length,
                &R2PutOptions::default(),
                None
            )
            .await
            .unwrap_err()
            .code(),
        ErrorCode::R2ObjectMetadataInvalid
    );
    let bad_version = R2UploadSource {
        path: path.clone(),
        length: valid.length,
        checksums: valid.checksums.clone(),
        version: "bad".to_owned(),
    };
    assert_eq!(
        store
            .put_file(&locator, &key, &bad_version, &R2PutOptions::default(), None)
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
                    checksum: Some(R2ChecksumAlgorithm::Md5([0; 16])),
                    ..R2PutOptions::default()
                },
                None,
            )
            .await
            .unwrap_err()
            .code(),
        ErrorCode::R2ChecksumMismatch
    );
    let missing_object = store
        .put_file(
            &locator,
            &key,
            &valid,
            &R2PutOptions {
                only_if: Some(R2Condition {
                    uploaded_before: Some(1),
                    ..R2Condition::default()
                }),
                ..R2PutOptions::default()
            },
            None,
        )
        .await
        .unwrap();
    assert!(missing_object.is_none());
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
    let source = upload_source(path, b"hello world");
    let key = UserObjectKey::parse("folder/a + %.txt").unwrap();
    let mut custom = BTreeMap::new();
    custom.insert("作者".to_owned(), "Elliot".to_owned());
    let options = R2PutOptions {
        http_metadata: R2HttpMetadata {
            content_type: Some("text/plain; charset=utf-8".to_owned()),
            cache_control: Some("max-age=60".to_owned()),
            ..R2HttpMetadata::default()
        },
        custom_metadata: custom.clone(),
        checksum: Some(R2ChecksumAlgorithm::Md5(source.checksums.md5)),
        ..R2PutOptions::default()
    };
    let put = store
        .put_file(&locator, &key, &source, &options, None)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(put.key, key.as_str());
    assert_eq!(put.custom_metadata.as_ref(), Some(&custom));
    assert_eq!(
        put.http_metadata
            .as_ref()
            .and_then(|meta| meta.content_type.as_ref()),
        options.http_metadata.content_type.as_ref()
    );
    assert_eq!(put.size, 11);
    assert_eq!(
        store.head(&locator, &key, None).await.unwrap(),
        Some(put.clone())
    );

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

    let failed = R2Condition {
        etag_matches: vec![R2EtagMatch::Strong {
            value: "different".to_owned(),
        }],
        ..R2Condition::default()
    };
    assert!(matches!(
        store
            .get(&locator, &key, None, Some(&failed), None)
            .await
            .unwrap(),
        R2GetResult::Precondition(_)
    ));
    store
        .delete(&locator, std::slice::from_ref(&key))
        .await
        .unwrap();
    assert!(store.head(&locator, &key, None).await.unwrap().is_none());
    assert!(store.is_empty(&locator).await.unwrap());
    store.delete_identity(&locator).await.unwrap();
    assert!(store.read_identity(&locator).await.unwrap().is_none());
}

#[tokio::test]
async fn conditional_put_rechecks_the_original_condition_after_a_create_race() {
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
    let temp = tempfile::tempdir().unwrap();
    let key = UserObjectKey::parse("conditional").unwrap();
    let source_path = temp.path().join("source");
    std::fs::write(&source_path, b"source").unwrap();
    let competitor_etag = hex::encode(hash_bytes(b"competitor").md5);

    mock.race_next_conditional_put(b"competitor".to_vec());
    let excluded = store
        .put_file(
            &locator,
            &key,
            &upload_source(source_path.clone(), b"source"),
            &R2PutOptions {
                only_if: Some(R2Condition {
                    etag_does_not_match: vec![R2EtagMatch::Strong {
                        value: competitor_etag.clone(),
                    }],
                    ..R2Condition::default()
                }),
                ..R2PutOptions::default()
            },
            None,
        )
        .await
        .unwrap();
    assert!(excluded.is_none());
    assert_eq!(
        store
            .head(&locator, &key, None)
            .await
            .unwrap()
            .unwrap()
            .etag,
        competitor_etag
    );

    store
        .delete(&locator, std::slice::from_ref(&key))
        .await
        .unwrap();
    mock.race_next_conditional_put(b"different".to_vec());
    let admitted = store
        .put_file(
            &locator,
            &key,
            &upload_source(source_path, b"source"),
            &R2PutOptions {
                only_if: Some(R2Condition {
                    etag_does_not_match: vec![R2EtagMatch::Strong {
                        value: "not-the-raced-etag".to_owned(),
                    }],
                    ..R2Condition::default()
                }),
                ..R2PutOptions::default()
            },
            None,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(admitted.etag, hex::encode(hash_bytes(b"source").md5));
    let physical_key = store.object_key(&locator, &key);
    let puts = mock
        .recorded()
        .into_iter()
        .filter(|request| request.method == "PUT" && request.path.ends_with(&physical_key))
        .collect::<Vec<_>>();
    assert_eq!(
        puts.len(),
        3,
        "the excluded race stops after its create fence; the admitted race retries once"
    );
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
                &UserObjectKey::parse(key).unwrap(),
                &upload_source(path, body),
                &R2PutOptions::default(),
                None,
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
                &UserObjectKey::parse(key).unwrap(),
                None,
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

#[path = "r2_tests/provider_multipart.rs"]
mod provider_multipart;
