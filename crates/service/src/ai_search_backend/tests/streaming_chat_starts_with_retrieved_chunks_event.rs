use super::*;

#[test]
fn streaming_chat_starts_with_retrieved_chunks_event() {
    let event = chunks_sse_event(&json!([{"id": "chunk-1", "score": 0.9}])).unwrap();
    assert_eq!(
        event,
        Bytes::from_static(b"event: chunks\ndata: [{\"id\":\"chunk-1\",\"score\":0.9}]\n\n")
    );
}
