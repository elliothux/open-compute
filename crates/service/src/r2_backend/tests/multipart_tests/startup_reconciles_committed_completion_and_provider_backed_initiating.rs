use super::*;

#[tokio::test]
async fn startup_reconciles_committed_completion_and_provider_backed_initiating() {
    let fixture = fixture().await;
    let account = fixture.storage.identity().default_account_id;
    let bucket = R2BucketRepository::new(fixture.storage.db())
        .get(account, fixture.resource)
        .unwrap();
    let locator = fixture
        .objects
        .locator(bucket.resource.id, &bucket.physical_prefix)
        .unwrap();
    let repo = R2MultipartRepository::new(fixture.storage.db());

    let created = fixture
        .service
        .handle(request(
            &fixture,
            "createMultipartUpload",
            JSON_CONTENT_TYPE,
            Body::from(r#"{"key":"restart-complete","options":{}}"#),
        ))
        .await;
    let upload_id = body_json(created).await["uploadId"]
        .as_str()
        .unwrap()
        .to_owned();
    let uploaded = fixture
        .service
        .handle(request(
            &fixture,
            "uploadPart",
            FRAME_CONTENT_TYPE,
            part_frame("restart-complete", &upload_id, 1, b"body", None),
        ))
        .await;
    let uploaded = body_json(uploaded).await;
    let parts = vec![open_compute_artifacts::R2UploadedPart {
        part_number: 1,
        etag: uploaded["etag"].as_str().unwrap().to_owned(),
    }];
    let open = repo
        .get(account, fixture.resource, &upload_id)
        .unwrap()
        .unwrap();
    R2ObjectRepository::new(fixture.storage.db())
        .begin_put(
            &R2ObjectRecord {
                resource_id: fixture.resource,
                account_id: account,
                object_key: "restart-complete".to_owned(),
                object_version: open.object_version.clone(),
                ssec_key_md5: None,
                ssec_envelope: None,
            },
            99,
        )
        .unwrap();
    let record = repo
        .begin_complete(
            account,
            fixture.resource,
            &upload_id,
            "restart-complete",
            &serde_json::to_string(&parts).unwrap(),
            100,
        )
        .unwrap();
    let key = UserObjectKey::parse("restart-complete").unwrap();
    fixture
        .objects
        .complete_multipart_upload(
            &locator,
            &key,
            record.provider_upload_id.as_deref().unwrap(),
            &parts,
            None,
        )
        .await
        .unwrap();
    assert_eq!(
        repo.get(account, fixture.resource, &upload_id)
            .unwrap()
            .unwrap()
            .state,
        R2MultipartState::Completing
    );
    multipart::reconcile_bucket_multipart(
        &fixture.storage,
        &fixture.objects,
        &bucket,
        true,
        false,
        Duration::from_secs(1),
    )
    .await
    .unwrap();
    assert_eq!(
        repo.get(account, fixture.resource, &upload_id)
            .unwrap()
            .unwrap()
            .state,
        R2MultipartState::Completed
    );

    let initiating = fixture
        .service
        .handle(request(
            &fixture,
            "createMultipartUpload",
            JSON_CONTENT_TYPE,
            Body::from(r#"{"key":"restart-init","options":{}}"#),
        ))
        .await;
    let initiating_id = body_json(initiating).await["uploadId"]
        .as_str()
        .unwrap()
        .to_owned();
    rusqlite::Connection::open(fixture._temp.path().join("data/control.sqlite"))
        .unwrap()
        .execute(
            "UPDATE r2_multipart_uploads SET state = 'initiating' WHERE upload_id = ?1",
            [&initiating_id],
        )
        .unwrap();
    multipart::reconcile_bucket_multipart(
        &fixture.storage,
        &fixture.objects,
        &bucket,
        true,
        false,
        Duration::from_secs(1),
    )
    .await
    .unwrap();
    assert_eq!(
        repo.get(account, fixture.resource, &initiating_id)
            .unwrap()
            .unwrap()
            .state,
        R2MultipartState::Aborted
    );
}
