use super::*;

#[test]
fn response_shapes_are_bounded_and_cloudflare_facing() {
    let item = AiSearchItemRecord {
        id: "item".to_owned(),
        key: "guide.txt".to_owned(),
        status: "completed".to_owned(),
        active_generation: Some(1),
        desired_generation: 1,
        metadata_json: br#"{"language":"en"}"#.to_vec(),
        created_at_ms: 10,
        updated_at_ms: 20,
        source_kind: "builtin".to_owned(),
        source: AiSearchSourceReference::Builtin(AiSearchObjectReference {
            object_key: "system/ai-search/object".to_owned(),
            object_sha256: [7; 32],
            object_size: 5,
        }),
        content_type: "text/plain".to_owned(),
        chunks_count: 2,
    };
    let value = item_info_value(&item).unwrap();
    assert_eq!(value["status"], "completed");
    assert_eq!(value["metadata"]["language"], "en");
    assert_eq!(page_bounds(Some(2), Some(10), 15).unwrap(), (2, 10, 10, 15));
    assert_eq!(
        metric_operation("namespace.search"),
        AiSearchOperation::Search
    );
}

#[tokio::test]
async fn item_filters_and_ordering_apply_before_pagination() {
    let fixture = SearchBehaviorFixture::create().await;
    let record = fixture.create_instance("ordered");
    fixture.seed_item(
        &record,
        "completed",
        "completed.txt",
        br#"{"category":"guide","rank":2}"#,
        &[("completed-chunk", "complete")],
    );
    let (store, inspection) = fixture.service.open_store(&record).unwrap();
    store
        .enqueue_item_generation(
            "queued-job",
            &NewAiSearchItemGeneration {
                item_id: "queued",
                key: "queued.txt",
                source: "builtin",
                generation: 1,
                index_generation: inspection.active_index_generation,
                object_key: "ai-search/v1/test/queued",
                object_sha256: [9; 32],
                object_size: 6,
                content_type: "text/plain",
                metadata_json: br#"{"category":"note","rank":1}"#,
                now_ms: 1,
            },
        )
        .unwrap();

    let list = |payload| {
        fixture.service.items_list(
            &fixture.namespace_authority(),
            JsonCall {
                operation: "items.list".to_owned(),
                instance: Some("ordered".to_owned()),
                payload,
            },
        )
    };
    let by_status = list(json!({"page":1,"per_page":1,"sort_by":"status"})).unwrap();
    assert_eq!(by_status["result"][0]["id"], "queued");
    assert_eq!(by_status["result_info"]["total_count"], 2);

    let by_modified = list(json!({"page":1,"per_page":1,"sort_by":"modified_at"})).unwrap();
    assert_eq!(by_modified["result"][0]["id"], "completed");

    let filtered = list(json!({
        "metadata_filter": "{\"rank\":{\"$gte\":2}}"
    }))
    .unwrap();
    assert_eq!(filtered["result_info"]["total_count"], 1);
    assert_eq!(filtered["result"][0]["id"], "completed");
}
