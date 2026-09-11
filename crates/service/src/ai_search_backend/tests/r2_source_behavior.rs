use super::*;

#[tokio::test]
async fn r2_source_reconciles_filters_metadata_exact_sync_and_download() {
    let fixture = SearchBehaviorFixture::create_with_r2().await;
    fixture
        .service
        .parser
        .parse_for_ai_search(
            fixture._runtime.account,
            "probe.md",
            "text/markdown",
            b"# parser probe".to_vec(),
        )
        .await
        .unwrap();
    let bucket = fixture.create_r2_bucket("source-bucket").await;
    fixture
        .put_r2_object(
            &bucket,
            "docs/guide.md",
            b"# R2 guide\n\nThe cobalt R2 marker is indexed.",
            BTreeMap::from([
                ("CATEGORY".to_owned(), "guide".to_owned()),
                ("rank".to_owned(), "7".to_owned()),
            ]),
        )
        .await;
    fixture
        .put_r2_object(&bucket, "docs/private.md", b"excluded", BTreeMap::new())
        .await;
    fixture
        .put_r2_object(&bucket, "docs/image.xyz", b"unsupported", BTreeMap::new())
        .await;
    fixture
        .put_r2_object(
            &bucket,
            "docs/ignored.bin",
            b"not included",
            BTreeMap::new(),
        )
        .await;
    fixture
        .put_r2_object(&bucket, "docs/empty.md", b"", BTreeMap::new())
        .await;

    let created = fixture
        .service
        .namespace_create(
            &fixture.namespace_authority(),
            JsonCall {
                operation: "namespace.create".to_owned(),
                instance: None,
                payload: json!({
                    "id": "r2-docs",
                    "type": "r2",
                    "source": "source-bucket",
                    "source_params": {
                        "prefix": "docs/",
                        "include_items": ["**/*.md", "**/*.xyz"],
                        "exclude_items": ["**/private*"]
                    },
                    "sync_interval": 900,
                    "index_method": {"vector": false, "keyword": true},
                    "indexing_options": {"keyword_tokenizer": "porter"},
                    "chunk": true,
                    "chunk_size": 32,
                    "chunk_overlap": 0,
                    "score_threshold": 0.0,
                    "max_num_results": 10,
                    "custom_metadata": [
                        {"field_name": "category", "data_type": "text"},
                        {"field_name": "rank", "data_type": "number"}
                    ]
                }),
            },
        )
        .unwrap();
    assert_eq!(created["type"], "r2");
    assert_eq!(created["source"], "source-bucket");
    assert_eq!(created["sync_interval"], 900);

    let record = AiSearchCatalog::new(fixture.storage().db())
        .get_instance_by_key(fixture._runtime.account, fixture.namespace.id, "r2-docs")
        .unwrap();
    assert_eq!(
        record.r2_source.as_ref().unwrap().bucket_resource_id,
        bucket.id
    );
    let (store, _) = fixture.service.open_store(&record).unwrap();
    fixture
        .service
        .run_r2_reconciler(&record, &store)
        .await
        .unwrap();

    let (items, total) = store.list_items(0, 100).unwrap();
    assert_eq!(total, 1);
    let item = &items[0];
    assert_eq!(item.key, "docs/guide.md");
    assert_eq!(item.source_kind, "r2");
    assert_eq!(
        serde_json::from_slice::<Value>(&item.metadata_json).unwrap(),
        json!({"category": "guide", "rank": 7})
    );
    assert!(matches!(item.source, AiSearchSourceReference::R2(_)));

    let AiSearchSourceReference::R2(source) = &item.source else {
        unreachable!()
    };
    let bucket_state = R2BucketRepository::new(fixture.storage().db())
        .get(fixture._runtime.account, bucket.id)
        .unwrap();
    let objects = r2_objects(&fixture._runtime._mock);
    let head = objects
        .head(
            &objects
                .locator(bucket.id, &bucket_state.physical_prefix)
                .unwrap(),
            &UserObjectKey::parse(&item.key).unwrap(),
            None,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(source.object_version, head.version);
    assert_eq!(source.etag, head.etag);
    assert_eq!(source.object_size, head.size);
    assert_eq!(source.uploaded_at_ms, head.uploaded);
    let conditional = open_compute_artifacts::R2Condition {
        etag_matches: vec![open_compute_artifacts::R2EtagMatch::Strong {
            value: source.etag.clone(),
        }],
        ..open_compute_artifacts::R2Condition::default()
    };
    assert!(matches!(
        objects
            .get(
                &objects
                    .locator(bucket.id, &bucket_state.physical_prefix)
                    .unwrap(),
                &UserObjectKey::parse(&item.key).unwrap(),
                None,
                Some(&conditional),
                None,
            )
            .await
            .unwrap(),
        R2GetResult::Body(_)
    ));
    assert_eq!(item.status, "completed");

    let (jobs, _) = store.list_jobs(0, 100).unwrap();
    let log_codes = jobs
        .iter()
        .flat_map(|job| store.job_logs(&job.id, 0, 100).unwrap())
        .map(|log| log.message_code)
        .collect::<BTreeSet<_>>();
    assert!(log_codes.contains("r2_skipped_by_exclude:1"));
    assert!(log_codes.contains("r2_skipped_by_include:1"));
    assert!(log_codes.contains("r2_skipped_unsupported_format:1"));
    assert!(log_codes.contains("r2_skipped_empty_or_oversize:1"));

    let response = fixture
        .service
        .download(
            fixture.namespace_authority(),
            ItemInput {
                instance: Some("r2-docs".to_owned()),
                item_id: item.id.clone(),
            },
        )
        .await
        .unwrap();
    assert_eq!(
        response.headers()["x-open-compute-filename"],
        "docs/guide.md"
    );
    assert_eq!(response.headers()[header::CONTENT_TYPE], "text/markdown");
    assert_eq!(
        to_bytes(response.into_body(), 1024).await.unwrap(),
        Bytes::from_static(b"# R2 guide\n\nThe cobalt R2 marker is indexed.")
    );

    let synced = fixture
        .service
        .item_sync(
            &fixture.namespace_authority(),
            JsonCall {
                operation: "item.sync".to_owned(),
                instance: Some("r2-docs".to_owned()),
                payload: json!({"itemId": item.id}),
            },
        )
        .await
        .unwrap();
    assert_eq!(synced["status"], "completed");
    assert_eq!(synced["source_id"], "source-bucket");

    store
        .enqueue_config_r2_reconcile(&Uuid::now_v7().to_string(), unix_ms())
        .unwrap();
    fixture
        .service
        .run_r2_reconciler(&record, &store)
        .await
        .unwrap();
    assert_eq!(store.list_items(0, 100).unwrap().1, 1);

    fixture
        .put_r2_object(
            &bucket,
            "docs/guide.md",
            b"# Updated R2 guide\n\nThe amber R2 marker replaced cobalt.",
            BTreeMap::from([
                ("category".to_owned(), "reference".to_owned()),
                ("rank".to_owned(), "not-a-number".to_owned()),
            ]),
        )
        .await;
    let changed = fixture
        .service
        .item_sync(
            &fixture.namespace_authority(),
            JsonCall {
                operation: "item.sync".to_owned(),
                instance: Some("r2-docs".to_owned()),
                payload: json!({"itemId": item.id}),
            },
        )
        .await
        .unwrap();
    assert_eq!(changed["status"], "completed");
    assert_eq!(changed["metadata"], json!({"category": "reference"}));
    let changed_item = store.get_item(&item.id).unwrap().unwrap();
    assert_eq!(changed_item.desired_generation, 2);
    assert!(
        store
            .active_chunks(Some(&item.id), 0, 100)
            .unwrap()
            .0
            .iter()
            .any(|chunk| chunk.text.contains("amber R2 marker"))
    );

    let interval = fixture
        .service
        .instance_update(
            &fixture.namespace_authority(),
            JsonCall {
                operation: "instance.update".to_owned(),
                instance: Some("r2-docs".to_owned()),
                payload: json!({"sync_interval": 1800}),
            },
        )
        .await
        .unwrap();
    assert_eq!(interval["sync_interval"], 1800);
    fixture
        .service
        .instance_update(
            &fixture.namespace_authority(),
            JsonCall {
                operation: "instance.update".to_owned(),
                instance: Some("r2-docs".to_owned()),
                payload: json!({
                    "source_params": {
                        "prefix": "docs/",
                        "include_items": ["**/*.md", "**/*.xyz"],
                        "exclude_items": ["**/private*", "**/guide.md"]
                    }
                }),
            },
        )
        .await
        .unwrap();
    assert_eq!(store.list_items(0, 100).unwrap().1, 0);
}
