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
