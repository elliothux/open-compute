use super::*;

#[tokio::test]
async fn object_authority_reconciles_every_current_put_and_delete_observation() {
    let fixture = fixture().await;
    let account = fixture.storage.identity().default_account_id;
    let bucket = R2BucketRepository::new(fixture.storage.db())
        .get(account, fixture.resource)
        .unwrap();
    let locator = fixture
        .objects
        .locator(bucket.resource.id, &bucket.physical_prefix)
        .unwrap();
    let binding = BindingRepository::new(fixture.storage.db())
        .authorize(fixture.binding, fixture.version, &fixture.descriptor)
        .unwrap();
    let repo = R2ObjectRepository::new(fixture.storage.db());
    let timeout = Duration::from_secs(1);

    let absent = UserObjectKey::parse("authority-absent").unwrap();
    assert!(
        fixture
            .service
            .committed_object(&binding, &locator, &absent, timeout)
            .await
            .unwrap()
            .is_none()
    );
    fixture
        .service
        .reconcile_object_key(&binding, &locator, &absent, timeout)
        .await
        .unwrap();
    let pending_absent = R2ObjectRecord {
        resource_id: fixture.resource,
        account_id: account,
        object_key: absent.as_str().to_owned(),
        object_version: uuid::Uuid::now_v7().to_string(),
        ssec_key_md5: None,
        ssec_envelope: None,
    };
    repo.begin_put(&pending_absent, 20).unwrap();
    assert_eq!(
        fixture
            .service
            .ensure_no_object_mutation(&binding, &absent)
            .unwrap_err()
            .code(),
        ErrorCode::R2ProviderUnavailable
    );
    fixture
        .service
        .reconcile_object_key(&binding, &locator, &absent, timeout)
        .await
        .unwrap();
    assert!(
        repo.get_mutation(account, fixture.resource, absent.as_str())
            .unwrap()
            .is_none()
    );

    let seeded = fixture
        .service
        .handle(request(
            &fixture,
            "put",
            FRAME_CONTENT_TYPE,
            put_frame("authority-existing", b"body", serde_json::json!({})),
        ))
        .await;
    assert_eq!(seeded.status(), StatusCode::OK);
    let key = UserObjectKey::parse("authority-existing").unwrap();
    let committed = repo
        .get(account, fixture.resource, key.as_str())
        .unwrap()
        .unwrap();
    let metadata = fixture
        .objects
        .head(&locator, &key, None)
        .await
        .unwrap()
        .unwrap();
    fixture
        .service
        .finish_object_put(&binding, &key, &metadata)
        .unwrap();
    let mut wrong = metadata.clone();
    wrong.key = "wrong".to_owned();
    assert_eq!(
        fixture
            .service
            .finish_object_put(&binding, &key, &wrong)
            .unwrap_err()
            .code(),
        ErrorCode::R2ObjectMetadataInvalid
    );
    assert_eq!(
        objects::validate_object_record(&committed, &wrong)
            .unwrap_err()
            .code(),
        ErrorCode::R2ObjectMetadataInvalid
    );

    fixture
        .service
        .begin_object_put(&binding, &key, &uuid::Uuid::now_v7().to_string(), None)
        .unwrap();
    fixture
        .service
        .reconcile_object_key(&binding, &locator, &key, timeout)
        .await
        .unwrap();
    assert!(
        repo.get_mutation(account, fixture.resource, key.as_str())
            .unwrap()
            .is_none()
    );

    let encrypted = fixture
        .service
        .handle(request(
            &fixture,
            "put",
            FRAME_CONTENT_TYPE,
            put_frame(
                "authority-encrypted",
                b"secret",
                serde_json::json!({"ssecKey": "cd".repeat(32)}),
            ),
        ))
        .await;
    assert_eq!(encrypted.status(), StatusCode::OK);
    let encrypted_key = UserObjectKey::parse("authority-encrypted").unwrap();
    let replacement_ssec = R2SsecKey::parse_hex(&"ef".repeat(32)).unwrap();
    fixture
        .service
        .begin_object_put(
            &binding,
            &encrypted_key,
            &uuid::Uuid::now_v7().to_string(),
            Some(&replacement_ssec),
        )
        .unwrap();
    fixture
        .service
        .reconcile_object_key(&binding, &locator, &encrypted_key, timeout)
        .await
        .unwrap();
    assert!(
        repo.get_mutation(account, fixture.resource, encrypted_key.as_str())
            .unwrap()
            .is_none()
    );

    fixture
        .service
        .begin_object_put(
            &binding,
            &encrypted_key,
            &uuid::Uuid::now_v7().to_string(),
            Some(&replacement_ssec),
        )
        .unwrap();
    fixture
        .mock
        .set_fault(open_compute_artifacts::Fault::ServerError);
    assert_eq!(
        fixture
            .service
            .reconcile_object_key(&binding, &locator, &encrypted_key, timeout)
            .await
            .unwrap_err()
            .code(),
        ErrorCode::R2ProviderUnavailable
    );
    fixture.mock.set_fault(open_compute_artifacts::Fault::None);
    repo.cancel_put(account, fixture.resource, encrypted_key.as_str())
        .unwrap();

    let ssec = R2SsecKey::parse_hex(&"ab".repeat(32)).unwrap();
    fixture
        .service
        .begin_object_put(
            &binding,
            &key,
            &uuid::Uuid::now_v7().to_string(),
            Some(&ssec),
        )
        .unwrap();
    fixture
        .service
        .reconcile_object_key(&binding, &locator, &key, timeout)
        .await
        .unwrap();
    assert!(
        repo.get_mutation(account, fixture.resource, key.as_str())
            .unwrap()
            .is_none()
    );

    repo.begin_delete(account, fixture.resource, &[key.as_str().to_owned()], 21)
        .unwrap();
    fixture
        .service
        .reconcile_object_key(&binding, &locator, &key, timeout)
        .await
        .unwrap();
    assert!(
        repo.get_mutation(account, fixture.resource, key.as_str())
            .unwrap()
            .is_none()
    );
    assert!(
        repo.get(account, fixture.resource, key.as_str())
            .unwrap()
            .is_some()
    );

    fixture
        .service
        .begin_object_put(&binding, &key, &uuid::Uuid::now_v7().to_string(), None)
        .unwrap();
    fixture
        .objects
        .delete(&locator, std::slice::from_ref(&key))
        .await
        .unwrap();
    assert_eq!(
        fixture
            .service
            .reconcile_object_key(&binding, &locator, &key, timeout)
            .await
            .unwrap_err()
            .code(),
        ErrorCode::R2ObjectMetadataInvalid
    );
    repo.cancel_put(account, fixture.resource, key.as_str())
        .unwrap();

    let inconsistent = R2ObjectRecord {
        ssec_key_md5: Some(ssec.md5_hex()),
        ..committed.clone()
    };
    assert_eq!(
        objects::open_object_ssec(&fixture.storage, &inconsistent)
            .unwrap_err()
            .code(),
        ErrorCode::R2ObjectMetadataInvalid
    );
    let (sealed_md5, sealed) =
        objects::seal_object_ssec(&fixture.storage, &binding, "sealed-version", Some(&ssec))
            .unwrap();
    let wrong_md5 = R2ObjectRecord {
        object_version: "sealed-version".to_owned(),
        ssec_key_md5: sealed_md5.map(|_| "wrong".to_owned()),
        ssec_envelope: sealed,
        ..committed
    };
    assert_eq!(
        objects::open_object_ssec(&fixture.storage, &wrong_md5)
            .unwrap_err()
            .code(),
        ErrorCode::R2ObjectMetadataInvalid
    );

    let batch = UserObjectKey::parse("authority-batch").unwrap();
    repo.begin_put(
        &R2ObjectRecord {
            resource_id: fixture.resource,
            account_id: account,
            object_key: batch.as_str().to_owned(),
            object_version: uuid::Uuid::now_v7().to_string(),
            ssec_key_md5: None,
            ssec_envelope: None,
        },
        22,
    )
    .unwrap();
    assert_eq!(
        objects::reconcile_bucket_objects(&fixture.storage, &fixture.objects, &bucket, timeout)
            .await
            .unwrap(),
        1
    );
}
