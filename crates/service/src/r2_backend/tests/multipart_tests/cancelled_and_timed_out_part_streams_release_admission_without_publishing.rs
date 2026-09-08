use super::*;

#[tokio::test]
async fn cancelled_and_timed_out_part_streams_release_admission_without_publishing() {
    use futures::StreamExt as _;
    let fixture = fixture().await;
    let created = fixture
        .service
        .handle(request(
            &fixture,
            "createMultipartUpload",
            JSON_CONTENT_TYPE,
            Body::from(r#"{"key":"interrupted","options":{}}"#),
        ))
        .await;
    assert_eq!(created.status(), StatusCode::OK);
    let created = body_json(created).await;
    let upload_id = created["uploadId"].as_str().unwrap();
    for cancel in [true, false] {
        let frame = to_bytes(
            part_frame("interrupted", upload_id, 1, b"unfinished", None),
            1024,
        )
        .await
        .unwrap();
        let stream = futures::stream::once(async move { Ok::<_, std::io::Error>(frame) })
            .chain(futures::stream::pending());
        let mut pending = Box::pin(fixture.service.handle(request(
            &fixture,
            "uploadPart",
            FRAME_CONTENT_TYPE,
            Body::from_stream(stream),
        )));
        if cancel {
            tokio::select! {
                _ = &mut pending => panic!("stream ended before cancellation"),
                () = async {
                    tokio::time::timeout(Duration::from_secs(2), async {
                        while fixture.service.staging_bytes.load(Ordering::Acquire) == 0 {
                            tokio::task::yield_now().await;
                        }
                    }).await.unwrap();
                } => {}
            }
            drop(pending);
        } else {
            let response = pending.await;
            assert_eq!(
                response.headers().get(ERROR_HEADER).unwrap(),
                ErrorCode::R2ProviderUnavailable.as_str()
            );
        }
        assert_eq!(fixture.service.staging_bytes.load(Ordering::Acquire), 0);
        assert_eq!(fixture.pins.count(fixture.resource), 0);
        assert!(
            std::fs::read_dir(fixture.storage.data_dir().root().join("r2-staging"))
                .unwrap()
                .next()
                .is_none()
        );
        let account = fixture.storage.identity().default_account_id;
        assert!(
            R2MultipartRepository::new(fixture.storage.db())
                .list_parts(upload_id)
                .unwrap()
                .is_empty()
        );
        assert!(
            R2ObjectRepository::new(fixture.storage.db())
                .get(account, fixture.resource, "interrupted")
                .unwrap()
                .is_none()
        );
        let lease = fixture
            .service
            .uploads
            .acquire(fixture.resource, Duration::from_millis(50))
            .await
            .unwrap();
        drop(lease);
    }
    let aborted = fixture
        .service
        .handle(request(
            &fixture,
            "abortMultipartUpload",
            JSON_CONTENT_TYPE,
            Body::from(
                serde_json::json!({"key": "interrupted", "uploadId": upload_id}).to_string(),
            ),
        ))
        .await;
    assert_eq!(aborted.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        R2MultipartRepository::new(fixture.storage.db())
            .get(
                fixture.storage.identity().default_account_id,
                fixture.resource,
                upload_id
            )
            .unwrap()
            .unwrap()
            .state,
        R2MultipartState::Aborted
    );
}
