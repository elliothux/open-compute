use super::*;

#[test]
fn upload_metadata_is_declared_string_input_and_materialized_by_schema() {
    let config: ResolvedAiSearchConfig = serde_json::from_value(json!({
        "id": "docs",
        "paused": false,
        "rewrite_query": false,
        "reranking": false,
        "embedding_model": "@cf/qwen/qwen3-embedding-0.6b",
        "index_method": {"vector": true, "keyword": true},
        "fusion_method": "rrf",
        "indexing_options": {"keyword_tokenizer": "porter"},
        "retrieval_options": {"keyword_match_mode": "and"},
        "chunk": true,
        "chunk_size": 64,
        "chunk_overlap": 0,
        "score_threshold": 0.4,
        "max_num_results": 10,
        "custom_metadata": [
            {"field_name": "rank", "data_type": "number"},
            {"field_name": "published", "data_type": "boolean"},
            {"field_name": "at", "data_type": "datetime"}
        ],
        "metadata": {}
    }))
    .unwrap();
    let input = json!({
        "rank": "2.5",
        "published": "true",
        "at": "2026-09-02T03:04:05Z"
    })
    .as_object()
    .unwrap()
    .clone();
    let value: Value =
        serde_json::from_slice(&materialize_upload_metadata(&config, &input).unwrap()).unwrap();
    assert_eq!(value["rank"], 2.5);
    assert_eq!(value["published"], true);
    assert_eq!(value["at"], "2026-09-02T03:04:05Z");

    let typed = json!({"rank": 2}).as_object().unwrap().clone();
    assert!(materialize_upload_metadata(&config, &typed).is_err());
    let undeclared = json!({"language": "en"}).as_object().unwrap().clone();
    assert!(materialize_upload_metadata(&config, &undeclared).is_err());
}
