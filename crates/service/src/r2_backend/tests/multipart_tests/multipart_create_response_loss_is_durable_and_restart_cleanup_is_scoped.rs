use super::*;

#[tokio::test]
async fn multipart_create_response_loss_is_durable_and_restart_cleanup_is_scoped() {
    let fixture = fixture().await;
    fixture
        .mock
        .set_fault(open_compute_artifacts::Fault::CreateResponseLoss);
    let response = fixture
        .service
        .handle(request(
            &fixture,
            "createMultipartUpload",
            JSON_CONTENT_TYPE,
            Body::from(r#"{"key":"lost-create","options":{}}"#),
        ))
        .await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        response.headers().get(ERROR_HEADER).unwrap(),
        ErrorCode::R2ResultUnknown.as_str()
    );
    assert!(fixture.mock.multipart_upload_count() >= 1);
    let account = fixture.storage.identity().default_account_id;
    let repo = R2MultipartRepository::new(fixture.storage.db());
    let rows = repo.list_for_resource(fixture.resource).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].state, R2MultipartState::CreateUnknown);
    assert!(rows[0].provider_upload_id.is_none());

    fixture.mock.set_fault(open_compute_artifacts::Fault::None);
    let bucket = R2BucketRepository::new(fixture.storage.db())
        .get(account, fixture.resource)
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
            >= 1
    );
    assert_eq!(fixture.mock.multipart_upload_count(), 0);
    assert_eq!(
        repo.list_for_resource(fixture.resource).unwrap()[0].state,
        R2MultipartState::Aborted
    );

    let open = fixture
        .service
        .handle(request(
            &fixture,
            "createMultipartUpload",
            JSON_CONTENT_TYPE,
            Body::from(r#"{"key":"delete-open","options":{}}"#),
        ))
        .await;
    assert_eq!(open.status(), StatusCode::OK);
    let open_id = body_json(open).await["uploadId"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(fixture.mock.multipart_upload_count() >= 1);
    multipart::reconcile_bucket_multipart(
        &fixture.storage,
        &fixture.objects,
        &bucket,
        false,
        true,
        Duration::from_secs(1),
    )
    .await
    .unwrap();
    assert_eq!(fixture.mock.multipart_upload_count(), 0);
    assert_eq!(
        repo.get(account, fixture.resource, &open_id)
            .unwrap()
            .unwrap()
            .state,
        R2MultipartState::Aborted
    );
}
