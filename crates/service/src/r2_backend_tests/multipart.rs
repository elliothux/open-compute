use super::*;

fn part_frame(
    key: &str,
    upload_id: &str,
    part_number: i32,
    bytes: &[u8],
    ssec_key: Option<&str>,
) -> Body {
    let header = serde_json::to_vec(&serde_json::json!({
        "key": key,
        "uploadId": upload_id,
        "partNumber": part_number,
        "ssecKey": ssec_key,
    }))
    .unwrap();
    let mut frame = u32::try_from(header.len()).unwrap().to_be_bytes().to_vec();
    frame.extend_from_slice(&header);
    frame.extend_from_slice(bytes);
    Body::from(frame)
}

#[tokio::test]
async fn private_protocol_covers_checksum_ssec_storage_class_multipart_and_start_after() {
    let fixture = fixture().await;
    let ssec = "ab".repeat(32);
    let mismatch = fixture
        .service
        .handle(request(
            &fixture,
            "put",
            FRAME_CONTENT_TYPE,
            put_frame(
                "bad-md5",
                b"hello",
                serde_json::json!({"checksum": {"algorithm": "md5", "hex": "00".repeat(16)}}),
            ),
        ))
        .await;
    assert_eq!(mismatch.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        mismatch.headers().get(ERROR_HEADER).unwrap(),
        ErrorCode::R2ChecksumMismatch.as_str()
    );

    let ia = fixture
        .service
        .handle(request(
            &fixture,
            "put",
            FRAME_CONTENT_TYPE,
            put_frame(
                "ia.bin",
                b"ia",
                serde_json::json!({"storageClass": "InfrequentAccess"}),
            ),
        ))
        .await;
    assert_eq!(ia.status(), StatusCode::OK);
    assert_eq!(body_json(ia).await["storageClass"], "InfrequentAccess");

    let ssec_put = fixture
        .service
        .handle(request(
            &fixture,
            "put",
            FRAME_CONTENT_TYPE,
            put_frame(
                "secret.bin",
                b"secret",
                serde_json::json!({"ssecKey": ssec}),
            ),
        ))
        .await;
    assert_eq!(ssec_put.status(), StatusCode::OK);
    let ssec_put = body_json(ssec_put).await;
    assert!(!ssec_put["ssecKeyMd5"].as_str().unwrap().is_empty());
    let restarted = R2BindingService::new(
        fixture.storage.clone(),
        fixture.pins.clone(),
        fixture.objects.clone(),
        R2Config {
            max_object_bytes: 1024 * 1024,
            max_staging_bytes: 2 * 1024 * 1024,
            operation_timeout_ms: 1000,
            ..R2Config::default()
        },
    )
    .unwrap();
    let ssec_head = restarted
        .handle(request(
            &fixture,
            "head",
            JSON_CONTENT_TYPE,
            Body::from(r#"{"key":"secret.bin"}"#),
        ))
        .await;
    assert_eq!(ssec_head.status(), StatusCode::OK);
    assert_eq!(
        body_json(ssec_head).await["ssecKeyMd5"],
        ssec_put["ssecKeyMd5"]
    );
    let denied = fixture
        .service
        .handle(request(
            &fixture,
            "get",
            FRAME_CONTENT_TYPE,
            Body::from(r#"{"key":"secret.bin","options":{}}"#),
        ))
        .await;
    assert_eq!(denied.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        denied.headers().get(ERROR_HEADER).unwrap(),
        ErrorCode::R2SsecInvalid.as_str()
    );
    let allowed = fixture
        .service
        .handle(request(
            &fixture,
            "get",
            FRAME_CONTENT_TYPE,
            Body::from(
                serde_json::json!({"key":"secret.bin","options":{"ssecKey": ssec}}).to_string(),
            ),
        ))
        .await;
    assert_eq!(allowed.status(), StatusCode::OK);

    let skipped = fixture
        .service
        .handle(request(
            &fixture,
            "put",
            FRAME_CONTENT_TYPE,
            put_frame(
                "ia.bin",
                b"nope",
                serde_json::json!({"onlyIf":{"etagMatches":[{"kind":"strong","value":"missing"}]}}),
            ),
        ))
        .await;
    assert_eq!(skipped.status(), StatusCode::NO_CONTENT);

    let listed = fixture
        .service
        .handle(request(
            &fixture,
            "list",
            JSON_CONTENT_TYPE,
            Body::from(r#"{"prefix":"","limit":1000,"include":[],"startAfter":"ia.bin"}"#),
        ))
        .await;
    let listed = body_json(listed).await;
    assert!(
        listed["objects"]
            .as_array()
            .unwrap()
            .iter()
            .all(|object| object["key"] != "ia.bin")
    );
    assert!(
        listed["objects"]
            .as_array()
            .unwrap()
            .iter()
            .any(|object| object["key"] == "secret.bin")
    );
    let ssec_md5 = ssec_put["ssecKeyMd5"].as_str().unwrap();
    let physical_suffix = hex::encode(Sha256::digest(b"secret.bin"));
    assert!(fixture.mock.recorded().iter().any(|request| {
        request.method == "HEAD"
            && request.path.ends_with(&physical_suffix)
            && request.ssec_algorithm.as_deref() == Some("AES256")
            && request.ssec_key_md5.as_deref() == Some(ssec_md5)
    }));

    let created = fixture
        .service
        .handle(request(
            &fixture,
            "createMultipartUpload",
            JSON_CONTENT_TYPE,
            Body::from(
                serde_json::json!({
                    "key": "mpu.txt",
                    "options": {"storageClass": "Standard", "ssecKey": ssec}
                })
                .to_string(),
            ),
        ))
        .await;
    assert_eq!(created.status(), StatusCode::OK);
    let created = body_json(created).await;
    let upload_id = created["uploadId"].as_str().unwrap().to_owned();
    let part = fixture
        .service
        .handle(request(
            &fixture,
            "uploadPart",
            FRAME_CONTENT_TYPE,
            part_frame("mpu.txt", &upload_id, 1, b"part-body", Some(&ssec)),
        ))
        .await;
    assert_eq!(part.status(), StatusCode::OK);
    let part = body_json(part).await;
    fixture
        .mock
        .set_fault(open_compute_artifacts::Fault::CompleteResponseLoss);
    let completed = fixture
        .service
        .handle(request(
            &fixture,
            "completeMultipartUpload",
            JSON_CONTENT_TYPE,
            Body::from(
                serde_json::json!({
                    "key": "mpu.txt",
                    "uploadId": upload_id,
                    "parts": [{"partNumber": 1, "etag": part["etag"]}]
                })
                .to_string(),
            ),
        ))
        .await;
    assert_eq!(completed.status(), StatusCode::OK);
    let completed = body_json(completed).await;
    assert_eq!(completed["key"], "mpu.txt");
    fixture.mock.set_fault(open_compute_artifacts::Fault::None);

    let other = fixture
        .service
        .handle(request(
            &fixture,
            "createMultipartUpload",
            JSON_CONTENT_TYPE,
            Body::from(r#"{"key":"abort.txt","options":{}}"#),
        ))
        .await;
    let other_id = body_json(other).await["uploadId"]
        .as_str()
        .unwrap()
        .to_owned();
    let aborted = fixture
        .service
        .handle(request(
            &fixture,
            "abortMultipartUpload",
            JSON_CONTENT_TYPE,
            Body::from(serde_json::json!({"key":"abort.txt","uploadId": other_id}).to_string()),
        ))
        .await;
    assert_eq!(aborted.status(), StatusCode::NO_CONTENT);
    let raced = fixture
        .service
        .handle(request(
            &fixture,
            "completeMultipartUpload",
            JSON_CONTENT_TYPE,
            Body::from(
                serde_json::json!({
                    "key": "mpu.txt",
                    "uploadId": upload_id,
                    "parts": [{"partNumber": 1, "etag": part["etag"]}]
                })
                .to_string(),
            ),
        ))
        .await;
    assert_eq!(raced.status(), StatusCode::OK);
    assert_eq!(body_json(raced).await, completed);

    let conflicting = fixture
        .service
        .handle(request(
            &fixture,
            "completeMultipartUpload",
            JSON_CONTENT_TYPE,
            Body::from(
                serde_json::json!({
                    "key": "mpu.txt",
                    "uploadId": upload_id,
                    "parts": []
                })
                .to_string(),
            ),
        ))
        .await;
    assert_eq!(conflicting.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        conflicting.headers().get(ERROR_HEADER).unwrap(),
        ErrorCode::R2MultipartInvalid.as_str()
    );

    rusqlite::Connection::open(fixture._temp.path().join("data/control.sqlite"))
        .unwrap()
        .execute(
            "UPDATE r2_multipart_uploads SET completion_manifest = ?1 WHERE upload_id = ?2",
            rusqlite::params![r#"[{"partNumber":1,"etag":"forged"}]"#, upload_id],
        )
        .unwrap();
    let forged = fixture
        .service
        .handle(request(
            &fixture,
            "completeMultipartUpload",
            JSON_CONTENT_TYPE,
            Body::from(
                serde_json::json!({
                    "key": "mpu.txt",
                    "uploadId": upload_id,
                    "parts": [{"partNumber": 1, "etag": "forged"}]
                })
                .to_string(),
            ),
        ))
        .await;
    assert_eq!(forged.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        forged.headers().get(ERROR_HEADER).unwrap(),
        ErrorCode::R2MultipartInvalid.as_str()
    );
}

#[test]
fn multipart_completion_enforces_uniform_nonfinal_parts_and_cloudflare_limits() {
    let requested = [
        open_compute_artifacts::R2UploadedPart {
            part_number: 1,
            etag: "one".to_owned(),
        },
        open_compute_artifacts::R2UploadedPart {
            part_number: 2,
            etag: "two".to_owned(),
        },
        open_compute_artifacts::R2UploadedPart {
            part_number: 3,
            etag: "three".to_owned(),
        },
    ];
    let base = open_compute_artifacts::R2_MIN_MULTIPART_PART_BYTES;
    let valid = [
        R2MultipartPartRecord {
            part_number: 1,
            etag: "one".to_owned(),
            size: base,
        },
        R2MultipartPartRecord {
            part_number: 2,
            etag: "two".to_owned(),
            size: base,
        },
        R2MultipartPartRecord {
            part_number: 3,
            etag: "three".to_owned(),
            size: 1,
        },
    ];
    multipart::validate_complete_parts(&requested, &valid).unwrap();

    let mut uneven = valid.clone();
    uneven[1].size += 1;
    assert_eq!(
        multipart::validate_complete_parts(&requested, &uneven)
            .unwrap_err()
            .code(),
        ErrorCode::R2InvalidOptions
    );
    let one = [requested[0].clone()];
    let oversized = [R2MultipartPartRecord {
        part_number: 1,
        etag: "one".to_owned(),
        size: open_compute_artifacts::R2_MAX_MULTIPART_PART_BYTES + 1,
    }];
    assert_eq!(
        multipart::validate_complete_parts(&one, &oversized)
            .unwrap_err()
            .code(),
        ErrorCode::R2InvalidOptions
    );

    assert_eq!(
        multipart::validate_complete_parts(&[], &[])
            .unwrap_err()
            .code(),
        ErrorCode::R2InvalidOptions
    );
    let duplicate = [requested[0].clone(), requested[0].clone()];
    assert_eq!(
        multipart::validate_complete_parts(&duplicate, &valid)
            .unwrap_err()
            .code(),
        ErrorCode::R2InvalidOptions
    );
    assert_eq!(
        multipart::validate_complete_parts(&one, &[])
            .unwrap_err()
            .code(),
        ErrorCode::R2MultipartInvalid
    );

    let count = usize::try_from(
        open_compute_artifacts::R2_MAX_MULTIPART_OBJECT_BYTES
            / open_compute_artifacts::R2_MAX_MULTIPART_PART_BYTES
            + 1,
    )
    .unwrap();
    let requested = (1..=count)
        .map(|part_number| open_compute_artifacts::R2UploadedPart {
            part_number: i32::try_from(part_number).unwrap(),
            etag: part_number.to_string(),
        })
        .collect::<Vec<_>>();
    let stored = requested
        .iter()
        .map(|part| R2MultipartPartRecord {
            part_number: part.part_number,
            etag: part.etag.clone(),
            size: open_compute_artifacts::R2_MAX_MULTIPART_PART_BYTES,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        multipart::validate_complete_parts(&requested, &stored)
            .unwrap_err()
            .code(),
        ErrorCode::R2InvalidOptions
    );
}

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
