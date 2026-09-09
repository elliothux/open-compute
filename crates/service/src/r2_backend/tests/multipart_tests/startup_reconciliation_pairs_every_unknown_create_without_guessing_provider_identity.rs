use super::*;

#[tokio::test]
async fn startup_reconciliation_pairs_every_unknown_create_without_guessing_provider_identity() {
    let fixture = fixture().await;
    let account = fixture.storage.identity().default_account_id;
    let bucket = R2BucketRepository::new(fixture.storage.db())
        .get(account, fixture.resource)
        .unwrap();
    let repo = R2MultipartRepository::new(fixture.storage.db());
    let insert_unknown = |key: &str, now: i64| {
        let upload_id = uuid::Uuid::now_v7().hyphenated().to_string();
        repo.insert_initiating(
            &R2MultipartUploadRecord {
                upload_id: upload_id.clone(),
                resource_id: fixture.resource,
                account_id: account,
                object_key: key.to_owned(),
                provider_upload_id: None,
                storage_class: "Standard".to_owned(),
                http_metadata: "{}".to_owned(),
                custom_metadata: "{}".to_owned(),
                ssec_key_md5: None,
                ssec_envelope: None,
                object_version: uuid::Uuid::now_v7().hyphenated().to_string(),
                completion_manifest: None,
                completed_metadata: None,
                state: R2MultipartState::Initiating,
            },
            now,
        )
        .unwrap();
        repo.mark_create_unknown(account, fixture.resource, &upload_id, now + 1)
            .unwrap();
        upload_id
    };

    let absent = insert_unknown("absent-provider", 100);
    assert_eq!(
        multipart::reconcile_bucket_multipart(
            &fixture.storage,
            &fixture.objects,
            &bucket,
            false,
            false,
            Duration::from_secs(1),
        )
        .await
        .unwrap(),
        1
    );
    assert!(
        repo.get(account, fixture.resource, &absent)
            .unwrap()
            .is_none()
    );

    fixture
        .mock
        .set_fault(open_compute_artifacts::Fault::CreateResponseLoss);
    let create_lost = |key: &'static str| {
        fixture.service.handle(request(
            &fixture,
            "createMultipartUpload",
            JSON_CONTENT_TYPE,
            Body::from(format!(r#"{{"key":"{key}","options":{{}}}}"#)),
        ))
    };
    assert_eq!(
        create_lost("more-intents").await.status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    let extra_intent = insert_unknown("more-intents", 200);
    fixture.mock.set_fault(open_compute_artifacts::Fault::None);
    assert!(
        multipart::reconcile_bucket_multipart(
            &fixture.storage,
            &fixture.objects,
            &bucket,
            false,
            false,
            Duration::from_secs(1),
        )
        .await
        .unwrap()
            >= 2
    );
    assert!(
        repo.get(account, fixture.resource, &extra_intent)
            .unwrap()
            .is_none_or(|record| record.state == R2MultipartState::Aborted)
    );

    fixture
        .mock
        .set_fault(open_compute_artifacts::Fault::CreateResponseLoss);
    assert_eq!(
        create_lost("more-orphans").await.status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(
        create_lost("more-orphans").await.status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    fixture.mock.set_fault(open_compute_artifacts::Fault::None);
    let mut unknown = repo
        .list_for_resource(fixture.resource)
        .unwrap()
        .into_iter()
        .filter(|record| {
            record.object_key == "more-orphans" && record.state == R2MultipartState::CreateUnknown
        })
        .collect::<Vec<_>>();
    unknown.sort_by(|left, right| left.upload_id.cmp(&right.upload_id));
    assert_eq!(unknown.len(), 2);
    repo.delete_create_unknown(account, fixture.resource, &unknown[0].upload_id)
        .unwrap();
    assert!(
        multipart::reconcile_bucket_multipart(
            &fixture.storage,
            &fixture.objects,
            &bucket,
            false,
            false,
            Duration::from_secs(1),
        )
        .await
        .unwrap()
            >= 2
    );
    assert_eq!(fixture.mock.multipart_upload_count(), 0);
}
