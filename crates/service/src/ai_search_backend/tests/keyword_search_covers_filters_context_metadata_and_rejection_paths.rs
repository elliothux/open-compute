use super::*;

#[tokio::test]
async fn keyword_search_covers_filters_context_metadata_and_rejection_paths() {
    let fixture = SearchBehaviorFixture::create().await;
    let record = fixture.create_instance("docs");
    fixture.seed_item(
        &record,
        "guide",
        "guide.txt",
        br#"{"category":"guide","rank":2}"#,
        &[
            ("guide-0", "alpha beta first"),
            ("guide-1", "neighbor context"),
            ("guide-2", "alpha second"),
        ],
    );
    fixture.seed_item(
        &record,
        "note",
        "note.txt",
        br#"{"category":"note","rank":1}"#,
        &[("note-0", "alpha beta note")],
    );
    let authority = fixture.authority(record.resource.clone(), BindingKind::AiSearchInstance);

    let result = fixture
        .service
        .instance_search(
            &authority,
            search_call(
                None,
                json!({
                    "query": "alpha beta",
                    "ai_search_options": {"retrieval": {
                        "retrieval_type": "keyword",
                        "keyword_match_mode": "or",
                        "filters": {"category": "guide"},
                        "context_expansion": 1,
                        "max_num_results": 5,
                        "match_threshold": 0.0
                    }}
                }),
            ),
        )
        .await
        .unwrap();
    let chunks = result["chunks"].as_array().unwrap();
    assert_eq!(chunks.len(), 2);
    assert!(
        chunks
            .iter()
            .all(|chunk| chunk["item"]["metadata"]["category"] == "guide")
    );
    assert!(
        chunks
            .iter()
            .any(|chunk| chunk["text"].as_str().unwrap().contains("neighbor context"))
    );

    let metadata_only = fixture
        .service
        .instance_search(
            &authority,
            search_call(
                None,
                json!({
                    "messages": [{"role":"user","content":"alpha"}],
                    "ai_search_options": {"retrieval": {
                        "retrieval_type": "keyword",
                        "metadata_only": true,
                        "filters": {"rank": {"$gte": 2}}
                    }}
                }),
            ),
        )
        .await
        .unwrap();
    assert!(
        metadata_only["chunks"]
            .as_array()
            .unwrap()
            .iter()
            .all(|chunk| chunk["text"] == "")
    );

    for payload in [
        json!({"query":"alpha","ai_search_options":{"retrieval":{"retrieval_type":"vector"}}}),
        json!({"query":"alpha","ai_search_options":{"retrieval":{"retrieval_type":"unknown"}}}),
        json!({"query":"alpha","ai_search_options":{"retrieval":{"retrieval_type":"keyword","keyword_match_mode":"xor"}}}),
        json!({"query":"alpha","ai_search_options":{"retrieval":{"retrieval_type":"keyword","boost_by":{}}}}),
        json!({"query":"alpha","ai_search_options":{"retrieval":{"retrieval_type":"keyword","context_expansion":4}}}),
        json!({"query":"alpha","ai_search_options":{"retrieval":{"retrieval_type":"keyword","filters":{"undeclared":"x"}}}}),
        json!({"query":"","ai_search_options":{}}),
    ] {
        assert!(
            fixture
                .service
                .instance_search(&authority, search_call(None, payload))
                .await
                .is_err()
        );
    }
}
