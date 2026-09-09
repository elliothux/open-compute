use super::*;

#[test]
fn queue_producer_persists_v8_content_type() {
    let temp = tempfile::tempdir().unwrap();
    let store = open_store(&temp, 1);
    let queue_id = QueueId::generate();
    let projection = QueueProjection {
        queue_id,
        account_id: AccountId::generate(),
        lifecycle_generation: 1,
        config_generation: 1,
        config: crate::QueueConfig::default(),
        created_at_ms: 1,
        updated_at_ms: 1,
    };
    store.create_queue_projection(&projection).unwrap();
    let body = b"OCDVv8-body".to_vec();
    store
        .enqueue_queue(
            &QueueEnqueueRequest {
                queue_id,
                request_id: Uuid::now_v7(),
                output_gate: false,
                lifecycle_generation: 1,
                config_generation: 1,
                batch_delay_seconds: None,
                messages: vec![QueueMessageInput {
                    content_type: QueueContentType::V8,
                    body: body.clone(),
                    delay_seconds: Some(0),
                }],
            },
            1,
        )
        .unwrap();
    let connection = Connection::open(temp.path().join("scheduler.sqlite")).unwrap();
    let (content_type, persisted): (String, Vec<u8>) = connection
        .query_row(
            "SELECT content_type, body FROM queue_messages WHERE queue_id = ?1",
            [queue_id.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(content_type, "v8");
    assert_eq!(persisted, body);
}
