use super::*;

#[test]
fn metadata_materialization_enforces_declared_types_and_limits() {
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
            {"field_name": "text", "data_type": "text"},
            {"field_name": "number", "data_type": "number"},
            {"field_name": "boolean", "data_type": "boolean"},
            {"field_name": "datetime", "data_type": "datetime"}
        ],
        "metadata": {}
    }))
    .unwrap();
    let valid = json!({
        "text": "hello",
        "number": "-1.25",
        "boolean": "false",
        "datetime": "2026-09-02T03:04:05Z"
    });
    let canonical = materialize_upload_metadata(&config, valid.as_object().unwrap()).unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&canonical).unwrap(),
        json!({
            "boolean": false,
            "datetime": "2026-09-02T03:04:05Z",
            "number": -1.25,
            "text": "hello"
        })
    );
    for invalid in [
        json!({"number": "nan"}),
        json!({"boolean": "TRUE"}),
        json!({"datetime": "tomorrow"}),
        json!({"unknown": "value"}),
        json!({"text": 1}),
    ] {
        assert_eq!(
            materialize_upload_metadata(&config, invalid.as_object().unwrap())
                .unwrap_err()
                .code(),
            ErrorCode::BindingProtocolError
        );
    }
    let too_many = json!({"a":"1","b":"2","c":"3","d":"4","e":"5","f":"6"});
    assert_eq!(
        materialize_upload_metadata(&config, too_many.as_object().unwrap())
            .unwrap_err()
            .code(),
        ErrorCode::BindingLimitExceeded
    );
}
