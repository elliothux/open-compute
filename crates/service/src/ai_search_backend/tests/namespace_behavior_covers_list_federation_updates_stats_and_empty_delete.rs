use super::*;

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
