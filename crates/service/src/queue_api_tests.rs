use super::*;
use open_compute_storage::QueueConfig;

#[test]
fn readiness_and_internal_errors_are_fail_closed() {
    let mut queue = open_compute_storage::QueueRecord {
        id: QueueId::generate(),
        account_id: AccountId::generate(),
        name: "queue".to_owned(),
        state: QueueState::Ready,
        availability: QueueAvailability::Healthy,
        availability_code: None,
        lifecycle_generation: 1,
        config_generation: 1,
        delivery_paused: false,
        config: QueueConfig::default(),
        created_at_ms: 1,
        updated_at_ms: 1,
        deleted_at_ms: None,
    };
    assert!(require_ready(&queue).is_ok());
    queue.state = QueueState::Creating;
    assert_eq!(
        require_ready(&queue).unwrap_err().code(),
        ErrorCode::QueueConsumerNotReady
    );
    queue.state = QueueState::Ready;
    queue.availability = QueueAvailability::Degraded;
    assert_eq!(
        require_ready(&queue).unwrap_err().code(),
        ErrorCode::QueueConsumerNotReady
    );
    assert_eq!(
        projection_pending().code(),
        ErrorCode::QueueConsumerProjectionPending
    );
    assert_eq!(internal().code(), ErrorCode::Internal);
    assert!(now_ms().unwrap() > 0);
}
