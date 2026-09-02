use super::*;

#[tokio::test]
async fn fence_rejects_new_pins_and_waits_for_existing_pin() {
    let id = VersionId::generate();
    let pins = VersionPins::new();
    let pin = pins.pin(id).unwrap();
    let waiter = tokio::spawn({
        let pins = pins.clone();
        async move { pins.fence_and_wait(id, Duration::from_secs(1)).await }
    });
    tokio::task::yield_now().await;
    assert_eq!(pins.pin(id).unwrap_err().code(), ErrorCode::VersionNotReady);
    drop(pin);
    waiter.await.unwrap().unwrap();
    pins.retire_fence(id);
    assert_eq!(pins.count(id), 0);
    assert!(pins.pin(id).is_ok());
}

#[test]
fn unfence_empty_entry_and_released_drop_are_idempotent() {
    let pins = VersionPins::new();
    let version = VersionId::generate();
    {
        let mut entries = pins.inner.entries.lock().unwrap();
        entries.insert(
            version,
            Entry {
                count: 0,
                fenced: true,
                retained_until_restart: false,
            },
        );
    }
    pins.unfence(version);
    assert_eq!(pins.count(version), 0);
    drop(VersionPin {
        version_id: version,
        inner: pins.inner.clone(),
        released: true,
    });
    assert_eq!(pins.count(version), 0);
}

#[tokio::test]
async fn unobservable_background_execution_is_retained_until_process_restart() {
    let pins = VersionPins::new();
    let version = VersionId::generate();
    pins.retain_until_restart(version).unwrap();
    pins.retain_until_restart(version).unwrap();
    assert_eq!(pins.count(version), 1);
    assert_eq!(
        pins.fence_and_wait(version, Duration::from_millis(1))
            .await
            .unwrap_err()
            .code(),
        ErrorCode::VersionReferenced
    );
    pins.unfence(version);
    assert_eq!(pins.count(version), 1);
    pins.clear_generation_retentions();
    assert_eq!(pins.count(version), 0);
    assert!(pins.pin(version).is_ok());
}
