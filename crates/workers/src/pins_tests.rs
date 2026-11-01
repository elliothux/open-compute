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
                deployments: HashMap::new(),
                fenced: true,
                retained_until_restart: false,
                awaiting_drain: HashSet::new(),
            },
        );
    }
    pins.unfence(version);
    assert_eq!(pins.count(version), 0);
    drop(VersionPin {
        version_id: version,
        deployment_id: None,
        inner: pins.inner.clone(),
        released: true,
    });
    assert_eq!(pins.count(version), 0);
}

#[test]
fn deployment_pins_expose_only_live_attribution() {
    let pins = VersionPins::new();
    let version = VersionId::generate();
    let deployment = DeploymentId::generate();
    let first = pins.pin_deployment(version, deployment).unwrap();
    let second = pins.pin_deployment(version, deployment).unwrap();
    assert_eq!(pins.active_deployments(), vec![deployment]);
    drop(first);
    assert_eq!(pins.active_deployments(), vec![deployment]);
    drop(second);
    assert!(pins.active_deployments().is_empty());
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

#[tokio::test]
async fn drain_acknowledgements_release_only_the_exact_activation() {
    let pins = VersionPins::new();
    let version = VersionId::generate();
    let first = RequestId::generate();
    let second = RequestId::generate();
    pins.retain_until_drain(version, first).unwrap();
    pins.retain_until_drain(version, second).unwrap();
    drop(pins.pin(version).unwrap());
    assert_eq!(pins.count(version), 2);
    pins.release_drained(version, first);
    pins.release_drained(version, first);
    assert_eq!(pins.count(version), 1);
    assert_eq!(
        pins.fence_and_wait(version, Duration::from_millis(1))
            .await
            .unwrap_err()
            .code(),
        ErrorCode::VersionReferenced
    );
    assert_eq!(
        pins.retain_until_drain(version, RequestId::generate())
            .unwrap_err()
            .code(),
        ErrorCode::VersionNotReady
    );
    pins.release_drained(version, second);
    pins.fence_and_wait(version, Duration::from_secs(1))
        .await
        .unwrap();
    pins.retire_fence(version);
    assert_eq!(pins.count(version), 0);
}

#[test]
fn drain_acknowledgements_cannot_release_background_or_later_generation_work() {
    let pins = VersionPins::new();
    let version = VersionId::generate();
    let old = RequestId::generate();
    pins.retain_until_restart(version).unwrap();
    pins.retain_until_drain(version, old).unwrap();
    pins.release_drained(version, old);
    assert_eq!(pins.count(version), 1);
    pins.retain_until_drain(version, old).unwrap();
    pins.clear_generation_retentions();
    assert_eq!(pins.count(version), 0);
    let next = RequestId::generate();
    pins.retain_until_drain(version, next).unwrap();
    pins.release_drained(version, old);
    pins.unfence(version);
    assert_eq!(pins.count(version), 1);
    pins.release_drained(version, next);
    assert_eq!(pins.count(version), 0);
}
