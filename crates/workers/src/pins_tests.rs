use super::*;

#[tokio::test]
async fn fence_rejects_new_pins_and_waits_for_existing_pin() {
    let id = DeploymentId::generate();
    let pins = DeploymentPins::new();
    let pin = pins.pin(id).unwrap();
    let waiter = tokio::spawn({
        let pins = pins.clone();
        async move { pins.fence_and_wait(id, Duration::from_secs(1)).await }
    });
    tokio::task::yield_now().await;
    assert_eq!(
        pins.pin(id).unwrap_err().code(),
        ErrorCode::DeploymentNotReady
    );
    drop(pin);
    waiter.await.unwrap().unwrap();
    pins.retire_fence(id);
    assert_eq!(pins.count(id), 0);
    assert!(pins.pin(id).is_ok());
}

#[test]
fn unfence_empty_entry_and_released_drop_are_idempotent() {
    let pins = DeploymentPins::new();
    let deployment = DeploymentId::generate();
    {
        let mut entries = pins.inner.entries.lock().unwrap();
        entries.insert(
            deployment,
            Entry {
                count: 0,
                fenced: true,
            },
        );
    }
    pins.unfence(deployment);
    assert_eq!(pins.count(deployment), 0);
    drop(DeploymentPin {
        deployment_id: deployment,
        inner: pins.inner.clone(),
        released: true,
    });
    assert_eq!(pins.count(deployment), 0);
}
