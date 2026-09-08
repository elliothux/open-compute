use super::*;

#[tokio::test]
async fn backend_local_capacity_and_framing_failures_release_owned_resources() {
    let fixture = fixture().await;
    assert!(format!("{:?}", fixture.service).contains("R2BindingService"));

    let wrong_method = axum::extract::Request::builder()
        .method("GET")
        .uri(format!("/internal/bindings/v1/r2/{}/head", fixture.binding))
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        fixture.service.handle(wrong_method).await.status(),
        StatusCode::BAD_REQUEST
    );

    let oversized_header = Body::from(
        u32::try_from(MAX_METADATA_BYTES + 1)
            .unwrap()
            .to_be_bytes()
            .to_vec(),
    );
    let response = fixture
        .service
        .handle(request(
            &fixture,
            "put",
            FRAME_CONTENT_TYPE,
            oversized_header,
        ))
        .await;
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(fixture.pins.count(fixture.resource), 0);

    let response = fixture
        .service
        .handle(request(
            &fixture,
            "put",
            FRAME_CONTENT_TYPE,
            put_frame("large", &vec![0; 1024 * 1024 + 1], serde_json::json!({})),
        ))
        .await;
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(fixture.pins.count(fixture.resource), 0);

    let used = Arc::new(AtomicU64::new(0));
    let mut reservation = StagingReservation::new(used.clone(), 1, None);
    reservation.add(1).unwrap();
    assert_eq!(
        reservation.add(1).unwrap_err().code(),
        ErrorCode::R2Overloaded
    );
    drop(reservation);
    assert_eq!(used.load(Ordering::Acquire), 0);

    let gate = OperationGate::new(1);
    let held = gate
        .acquire(fixture.resource, Duration::from_secs(1))
        .await
        .unwrap();
    let saturated = gate
        .acquire(fixture.resource, Duration::from_millis(1))
        .await;
    assert!(matches!(saturated, Err(ref error) if error.code() == ErrorCode::R2Overloaded));
    drop(held);

    let seeded = fixture
        .service
        .handle(request(
            &fixture,
            "put",
            FRAME_CONTENT_TYPE,
            put_frame("corrupt", b"body", serde_json::json!({})),
        ))
        .await;
    assert_eq!(seeded.status(), StatusCode::OK);
    fixture
        .mock
        .set_fault(open_compute_artifacts::Fault::CorruptMetadata);
    let response = fixture
        .service
        .handle(request(
            &fixture,
            "head",
            JSON_CONTENT_TYPE,
            Body::from(serde_json::json!({"key": "corrupt"}).to_string()),
        ))
        .await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
}
