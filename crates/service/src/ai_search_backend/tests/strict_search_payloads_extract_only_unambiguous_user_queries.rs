use super::*;

#[test]
fn strict_search_payloads_extract_only_unambiguous_user_queries() {
    let direct: SearchPayload = serde_json::from_value(json!({"query": "needle"})).unwrap();
    assert_eq!(direct.query_text().unwrap(), "needle");

    let messages: SearchPayload = serde_json::from_value(json!({
        "messages": [
            {"role": "user", "content": "old"},
            {"role": "assistant", "content": "answer"},
            {"role": "user", "content": "new"}
        ]
    }))
    .unwrap();
    assert_eq!(messages.query_text().unwrap(), "new");
    for payload in [
        json!({}),
        json!({"query": ""}),
        json!({"query": "x", "messages": []}),
        json!({"messages": [{"role": "assistant", "content": "none"}]}),
        json!({"messages": [{"role": "user", "content": ""}]}),
    ] {
        let payload: SearchPayload = serde_json::from_value(payload).unwrap();
        assert_eq!(
            payload.query_text().unwrap_err().code(),
            ErrorCode::BindingProtocolError
        );
    }
    assert!(
        serde_json::from_value::<SearchPayload>(json!({
            "query": "x",
            "unexpected": true
        }))
        .is_err()
    );

    let chat: ChatPayload = serde_json::from_value(json!({
        "messages": [{"role": "user", "content": "chat query"}],
        "stream": true
    }))
    .unwrap();
    assert_eq!(chat.as_search().query_text().unwrap(), "chat query");
}
