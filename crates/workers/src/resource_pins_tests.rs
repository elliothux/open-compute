use super::*;

#[tokio::test]
async fn resource_fence_wait_timeout_unfence_and_retire() {
    let pins = ResourcePins::new();
    let id = ResourceId::generate();
    let pin = pins.try_pin(id).unwrap();
    assert_eq!(pins.count(id), 1);
    assert_eq!(
        pins.fence_and_wait(id, Duration::from_millis(1))
            .await
            .unwrap_err()
            .code(),
        ErrorCode::ResourceReferenced
    );
    assert_eq!(
        pins.try_pin(id).unwrap_err().code(),
        ErrorCode::ResourceNotReady
    );
    pins.unfence(id);
    let second = pins.try_pin(id).unwrap();
    drop(second);
    drop(pin);
    pins.fence_and_wait(id, Duration::from_secs(1))
        .await
        .unwrap();
    pins.retire_fence(id);
    assert_eq!(pins.count(id), 0);
    assert!(format!("{:?}", pins.try_pin(id).unwrap()).contains("ResourcePin"));
}
