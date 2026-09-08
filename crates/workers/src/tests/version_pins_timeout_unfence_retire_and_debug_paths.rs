use super::*;

#[tokio::test]
async fn version_pins_timeout_unfence_retire_and_debug_paths() {
    let id = VersionId::generate();
    let pins = VersionPins::new();
    assert_eq!(pins.count(id), 0);
    pins.unfence(id);
    pins.retire_fence(id);

    let pin = pins.pin(id).unwrap();
    assert_eq!(pins.count(id), 1);
    assert!(format!("{pin:?}").contains(&id.to_string()));
    assert_eq!(
        pins.fence_and_wait(id, Duration::ZERO)
            .await
            .unwrap_err()
            .code(),
        ErrorCode::VersionReferenced
    );
    pins.unfence(id);
    let second = pins.pin(id).unwrap();
    assert_eq!(pins.count(id), 2);
    drop(second);
    assert_eq!(pins.count(id), 1);
    drop(pin);
    assert_eq!(pins.count(id), 0);

    pins.fence_and_wait(id, Duration::from_millis(50))
        .await
        .unwrap();
    pins.retire_fence(id);
    assert!(pins.pin(id).is_ok());
}
