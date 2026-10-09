use super::*;

#[tokio::test]
async fn maintenance_waits_for_configuration_updates_before_opening_the_store() {
    let fixture = SearchBehaviorFixture::create().await;
    let record = fixture.create_instance("docs");
    let lock = fixture.service.generation_lock(record.resource.id).unwrap();
    let writer = lock.write_owned().await;
    let mut maintenance = Box::pin(fixture.service.maintenance_once());
    assert!(futures::poll!(maintenance.as_mut()).is_pending());
    let (store, inspection) = fixture.service.open_store(&record).unwrap();
    let mut config: Value = serde_json::from_slice(&inspection.public_config_json).unwrap();
    config["metadata"] = json!({"updated": true});
    assert!(
        store
            .update_public_config(
                inspection.config_generation,
                &serde_json::to_vec(&config).unwrap(),
                unix_ms(),
            )
            .unwrap()
    );
    drop(writer);
    tokio::time::timeout(Duration::from_secs(5), maintenance)
        .await
        .unwrap()
        .unwrap();
    let current = AiSearchCatalog::new(fixture.storage().db())
        .get_instance(record.resource.instance_id, record.resource.id)
        .unwrap();
    assert_eq!(current.resource.availability, ResourceAvailability::Healthy);
    assert_eq!(
        serde_json::from_slice::<Value>(&store.inspect().unwrap().public_config_json).unwrap(),
        config
    );
}

#[tokio::test]
async fn manual_source_is_visible_only_through_the_namespaced_extension() {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}/source", listener.local_addr().unwrap());
    let digest = hex::encode(Sha256::digest(b"alpha"));
    let provider = tokio::spawn(async move {
        for operation in ["resolve", "read", "read"] {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = vec![0; 4096];
            let count = stream.read(&mut request).await.unwrap();
            let request = std::str::from_utf8(&request[..count]).unwrap();
            assert!(request.starts_with(&format!("POST /source/{operation} HTTP/1.1")));
            assert!(request.contains("authorization: Bearer fixture-manual-token"));
            assert!(request.contains(r#""key":"files/guide.txt""#));
            assert!(request.contains(r#""revision":"rev-1""#));
            let response = if operation == "resolve" {
                let body = format!(
                    r#"{{"revision":"rev-1","contentType":"text/plain","size":5,"sha256":"{digest}"}}"#
                );
                format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                )
            } else {
                format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: text/plain\r\nx-open-compute-revision: rev-1\r\nx-open-compute-size: 5\r\nx-open-compute-sha256: {digest}\r\ncontent-length: 5\r\nconnection: close\r\n\r\nalpha"
                )
            };
            stream.write_all(response.as_bytes()).await.unwrap();
        }
    });
    let fixture = SearchBehaviorFixture::create_with_manual_provider(endpoint).await;
    let namespace = fixture.namespace_authority();
    let config = json!({
        "id": "manual",
        "embedding_model": "@cf/qwen/qwen3-embedding-0.6b",
        "index_method": {"vector": false, "keyword": true},
        "indexing_options": {"keyword_tokenizer": "porter"},
        "retrieval_options": {"keyword_match_mode": "and"}
    });
    let created = fixture
        .service
        .namespace_open_compute_create_manual(
            &namespace,
            JsonCall {
                operation: "namespace.openComputeCreateManual".to_owned(),
                instance: None,
                payload: json!({"providerId": "fixture-manual", "config": config}),
            },
        )
        .unwrap();
    assert_eq!(created["type"], "open-compute:manual");
    assert_eq!(
        created["open_compute_source"]["provider_id"],
        "fixture-manual"
    );
    assert_eq!(created["open_compute_source"]["source"], "fixture-files");

    let upserted = fixture
        .service
        .manual_upsert(
            &namespace,
            JsonCall {
                operation: "items.openComputeUpsert".to_owned(),
                instance: Some("manual".to_owned()),
                payload: json!({
                    "key": "files/guide.txt",
                    "revision": "rev-1",
                    "contentType": "text/plain",
                    "metadata": {},
                    "waitForCompletion": true
                }),
            },
        )
        .await
        .unwrap();
    assert_eq!(upserted["status"], "completed");
    let item_id = upserted["id"].as_str().unwrap().to_owned();
    let result = fixture
        .service
        .instance_search(
            &namespace,
            search_call(
                Some("manual"),
                json!({
                    "query": "alpha",
                    "ai_search_options": {"retrieval": {"retrieval_type": "keyword"}}
                }),
            ),
        )
        .await
        .unwrap();
    let source = &result["chunks"][0]["item"]["open_compute_source"];
    assert_eq!(source["provider_id"], "fixture-manual");
    assert_eq!(source["source"], "fixture-files");
    assert_eq!(source["key"], "files/guide.txt");
    assert_eq!(source["revision"], "rev-1");

    let downloaded = fixture
        .service
        .download(
            fixture.namespace_authority(),
            ItemInput {
                instance: Some("manual".to_owned()),
                item_id,
            },
        )
        .await
        .unwrap();
    assert_eq!(downloaded.headers()[header::CONTENT_TYPE], "text/plain");
    assert_eq!(
        downloaded.headers()["x-open-compute-source-provider"],
        "fixture-manual"
    );
    assert_eq!(
        to_bytes(downloaded.into_body(), 16).await.unwrap(),
        Bytes::from_static(b"alpha")
    );
    provider.await.unwrap();

    let mut official = fixture.namespace_authority();
    official.allow_extensions = false;
    let listed = fixture
        .service
        .namespace_list(
            &official,
            JsonCall {
                operation: "namespace.list".to_owned(),
                instance: None,
                payload: json!({}),
            },
        )
        .unwrap();
    assert_eq!(listed["result"], json!([]));
    assert!(
        fixture
            .service
            .resolve_instance(&official, Some("manual"))
            .is_err()
    );
    assert!(
        fixture
            .service
            .namespace_open_compute_create_manual(
                &official,
                JsonCall {
                    operation: "namespace.openComputeCreateManual".to_owned(),
                    instance: None,
                    payload: json!({"providerId": "fixture-manual", "config": {}}),
                },
            )
            .is_err()
    );
}

#[tokio::test]
async fn namespace_behavior_covers_list_federation_updates_stats_and_empty_delete() {
    let fixture = SearchBehaviorFixture::create().await;
    let docs = fixture.create_instance("docs");
    let archive = fixture.create_instance("archive");
    let _disposable = fixture.create_instance("disposable");
    fixture.seed_item(
        &docs,
        "docs-item",
        "docs.txt",
        br#"{"category":"guide","rank":2}"#,
        &[("docs-0", "alpha docs")],
    );
    fixture.seed_item(
        &archive,
        "archive-item",
        "archive.txt",
        br#"{"category":"note","rank":1}"#,
        &[("archive-0", "alpha archive")],
    );
    let namespace = fixture.namespace_authority();

    let listed = fixture
        .service
        .namespace_list(
            &namespace,
            JsonCall {
                operation: "namespace.list".to_owned(),
                instance: None,
                payload: json!({
                    "page": 1,
                    "per_page": 2,
                    "search": "a",
                    "order_by": "created_at",
                    "order_by_direction": "desc"
                }),
            },
        )
        .unwrap();
    assert_eq!(listed["result"].as_array().unwrap().len(), 2);
    assert_eq!(listed["result_info"]["total_count"], 2);

    for payload in [
        json!({"order_by":"name"}),
        json!({"order_by_direction":"sideways"}),
        json!({"page":0}),
    ] {
        assert!(
            fixture
                .service
                .namespace_list(
                    &namespace,
                    JsonCall {
                        operation: "namespace.list".to_owned(),
                        instance: None,
                        payload,
                    },
                )
                .is_err()
        );
    }

    let federated = fixture
        .service
        .namespace_search(
            &namespace,
            JsonCall {
                operation: "namespace.search".to_owned(),
                instance: None,
                payload: json!({
                    "query":"alpha",
                    "ai_search_options": {
                        "instance_ids":["docs","archive","missing"],
                        "retrieval":{"retrieval_type":"keyword","return_on_failure":true}
                    }
                }),
            },
        )
        .await
        .unwrap();
    assert_eq!(federated["chunks"].as_array().unwrap().len(), 2);
    assert_eq!(federated["errors"][0]["instance_id"], "missing");
    assert!(
        federated["chunks"]
            .as_array()
            .unwrap()
            .iter()
            .all(|chunk| chunk.get("instance_id").is_some())
    );

    for payload in [
        json!({"query":"alpha","ai_search_options":{"instance_ids":[]}}),
        json!({"query":"alpha","ai_search_options":{"instance_ids":["docs","docs"]}}),
        json!({"query":"alpha","ai_search_options":{"instance_ids":["missing"],"retrieval":{"return_on_failure":false}}}),
    ] {
        assert!(
            fixture
                .service
                .namespace_search(
                    &namespace,
                    JsonCall {
                        operation: "namespace.search".to_owned(),
                        instance: None,
                        payload,
                    },
                )
                .await
                .is_err()
        );
    }

    let docs_authority = fixture.authority(docs.resource.clone(), BindingKind::AiSearchInstance);
    let info = fixture
        .service
        .instance_info_call(
            &docs_authority,
            &JsonCall {
                operation: "instance.info".to_owned(),
                instance: None,
                payload: json!({}),
            },
        )
        .unwrap();
    assert_eq!(info["status"], "ready");
    let stats = fixture
        .service
        .instance_stats(
            &docs_authority,
            &JsonCall {
                operation: "instance.stats".to_owned(),
                instance: None,
                payload: json!({}),
            },
        )
        .unwrap();
    assert_eq!(stats["completed"], 1);
    assert_eq!(stats["engine"]["chunks"], 1);
    let updatable = fixture.create_instance_with_vector("updatable", true);
    let updatable_authority =
        fixture.authority(updatable.resource.clone(), BindingKind::AiSearchInstance);
    let updated = fixture
        .service
        .instance_update(
            &updatable_authority,
            JsonCall {
                operation: "instance.update".to_owned(),
                instance: None,
                payload: json!({"metadata":{"updated":true}}),
            },
        )
        .await
        .unwrap();
    assert_eq!(updated["metadata"]["updated"], true);

    let reindexed = fixture
        .service
        .instance_update(
            &updatable_authority,
            JsonCall {
                operation: "instance.update".to_owned(),
                instance: None,
                payload: json!({"chunk_size":16}),
            },
        )
        .await
        .unwrap();
    assert_eq!(reindexed["chunk_size"], 16);
    let whole_document = fixture
        .service
        .instance_update(
            &updatable_authority,
            JsonCall {
                operation: "instance.update".to_owned(),
                instance: None,
                payload: json!({"chunk":false}),
            },
        )
        .await
        .unwrap();
    assert_eq!(whole_document["chunk"], false);
    let metadata_after_chunk_change = fixture
        .service
        .instance_update(
            &updatable_authority,
            JsonCall {
                operation: "instance.update".to_owned(),
                instance: None,
                payload: json!({"metadata":{"chunk":false}}),
            },
        )
        .await
        .unwrap();
    assert_eq!(metadata_after_chunk_change["chunk"], false);
    drop(updatable_authority);
    let deleted = fixture
        .service
        .namespace_delete(
            &namespace,
            JsonCall {
                operation: "namespace.delete".to_owned(),
                instance: None,
                payload: json!({"instance":"disposable"}),
            },
        )
        .await
        .unwrap();
    assert_eq!(deleted, Value::Null);
}
