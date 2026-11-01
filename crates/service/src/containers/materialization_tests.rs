use super::{test_support::*, *};
use open_compute_artifacts::ArtifactRef;
use std::sync::atomic::Ordering;

#[tokio::test]
async fn admitted_image_materializes_only_on_cache_absence_and_survives_service_restart() {
    let fixture = fixture().await;
    let image = fixture.admit_image("cached-image").await;
    let repo = ContainerAuthority::new(fixture.service.storage.db());
    let metadata = repo.image(fixture.service.instance_id(), &image).unwrap();
    let image_id = format!("sha256:{}", hex::encode(metadata.config_sha256));
    assert_eq!(fixture.protocol.imports.load(Ordering::SeqCst), 0);
    let authority = authority(&fixture.service, &fixture._artifacts, image, true);
    let patch=serde_json::from_value(serde_json::json!({"configuration":{
        "image":format!("registry.example.test/{}/authority-app@sha256:{}",fixture.service.instance_id(),hex::encode(image)),
        "instance_type":"lite"}})).unwrap();
    fixture
        .service
        .update_application(authority.app.id, &patch, 9)
        .await
        .unwrap();
    assert_eq!(fixture.protocol.imports.load(Ordering::SeqCst), 0);
    assert!(fixture.protocol.images.lock().await.is_empty());
    assert_eq!(fixture.service.materialize_image(&image).await.unwrap(), 0);
    assert_eq!(fixture.protocol.imports.load(Ordering::SeqCst), 1);
    let rootfs = fixture.protocol.images.lock().await[&image_id].clone();
    fixture.protocol.image_size.store(173, Ordering::SeqCst);
    assert_eq!(
        fixture.service.materialize_image(&image).await.unwrap(),
        173
    );
    assert_eq!(
        fixture
            .restart_service()
            .materialize_image(&image)
            .await
            .unwrap(),
        173
    );
    assert_eq!(fixture.protocol.imports.load(Ordering::SeqCst), 1);
    fixture.protocol.images.lock().await.remove(&image_id);
    assert_eq!(
        fixture
            .restart_service()
            .materialize_image(&image)
            .await
            .unwrap(),
        173
    );
    assert_eq!(fixture.protocol.imports.load(Ordering::SeqCst), 2);
    assert_eq!(fixture.protocol.images.lock().await[&image_id], rootfs);
    assert!(
        fixture
            .protocol
            .image_checks
            .lock()
            .await
            .iter()
            .all(|id| id == &image_id)
    );
    fixture
        .protocol
        .images
        .lock()
        .await
        .insert(image_id.clone(), vec![format!("sha256:{}", "0".repeat(64))]);
    assert!(fixture.service.materialize_image(&image).await.is_err());
    assert_eq!(fixture.protocol.imports.load(Ordering::SeqCst), 2);
    fixture
        .protocol
        .images
        .lock()
        .await
        .insert(image_id, rootfs);

    fixture
        .service
        .resolve(
            authority.request(true, None),
            10,
            "container-unit-generation",
        )
        .await
        .unwrap();
    assert_eq!(fixture.protocol.imports.load(Ordering::SeqCst), 2);
    let expected = metadata
        .diff_ids
        .iter()
        .map(|id| format!("sha256:{}", hex::encode(id)))
        .collect::<Vec<_>>();
    assert_eq!(
        fixture
            .protocol
            .registrations
            .lock()
            .await
            .last()
            .unwrap()
            .diff_ids,
        expected
    );
}

#[tokio::test]
async fn missing_image_import_failure_preserves_reservation_and_retry_identity() {
    let fixture = fixture().await;
    let image = fixture.admit_image("failed-import").await;
    let authority = authority(&fixture.service, &fixture._artifacts, image, true);

    fixture.protocol.import_fail.store(true, Ordering::SeqCst);
    assert!(
        fixture
            .service
            .resolve(
                authority.request(true, None),
                10,
                "container-unit-generation"
            )
            .await
            .is_err()
    );
    let repo = ContainerAuthority::new(fixture.service.storage.db());
    let first = repo
        .application_leases(fixture.service.instance_id(), authority.app.id)
        .unwrap()
        .pop()
        .unwrap();
    assert_eq!(first.state, ContainerLeaseState::Admitting);
    assert!(fixture.protocol.registrations.lock().await.is_empty());
    let mut other = authority.request(true, None);
    other.object_id = insert_object(&fixture.service, authority.object.namespace_resource_id, 8);
    assert!(
        fixture
            .service
            .resolve(other, 11, "container-unit-generation")
            .await
            .is_err()
    );
    assert_eq!(fixture.protocol.imports.load(Ordering::SeqCst), 1);
    assert_eq!(
        fixture
            .service
            .storage
            .admission_snapshot()
            .unwrap()
            .reserved_bytes,
        0
    );
    fixture.protocol.import_fail.store(false, Ordering::SeqCst);
    fixture.protocol.register_fail.store(true, Ordering::SeqCst);
    assert!(
        fixture
            .restart_service()
            .resolve(
                authority.request(true, None),
                12,
                "container-unit-generation"
            )
            .await
            .is_err()
    );
    let lost = repo
        .application_leases(fixture.service.instance_id(), authority.app.id)
        .unwrap()
        .pop()
        .unwrap();
    assert_eq!(lost.opaque_engine_key, first.opaque_engine_key);
    assert_eq!(lost.identity, first.identity);
    assert_eq!(lost.state, ContainerLeaseState::Lost);
    assert_eq!(fixture.protocol.imports.load(Ordering::SeqCst), 2);
    fixture
        .protocol
        .register_fail
        .store(false, Ordering::SeqCst);
    let recovered = fixture
        .restart_service()
        .resolve(
            authority.request(true, None),
            13,
            "container-unit-generation",
        )
        .await
        .unwrap()
        .lease
        .unwrap();
    assert_eq!(recovered.opaque_engine_key, first.opaque_engine_key);
    assert_eq!(fixture.protocol.imports.load(Ordering::SeqCst), 2);
    assert_eq!(
        fixture
            .service
            .storage
            .admission_snapshot()
            .unwrap()
            .reserved_bytes,
        0
    );
    assert_eq!(
        repo.application_leases(fixture.service.instance_id(), authority.app.id)
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn missing_image_disk_admission_precedes_cache_download_and_releases_reservation() {
    let fixture = fixture().await;
    let digest = fixture.admit_image("disk-admission").await;
    let image = ContainerAuthority::new(fixture.service.storage.db())
        .image(fixture.service.instance_id(), &digest)
        .unwrap();
    let artifact = ArtifactRef::new(
        open_compute_artifacts::ARTIFACT_KEY_VERSION,
        &hex::encode(image.artifact_sha256),
        image.artifact_size,
    )
    .unwrap();
    assert!(
        fixture
            .service
            .cache
            .acquire_cached(&artifact)
            .await
            .is_err()
    );
    let gets = fixture._artifacts._mock.artifact_gets();
    let reserved = fixture
        .service
        .storage
        .admission_snapshot()
        .unwrap()
        .reserved_bytes;
    fixture.service.storage.begin_draining();
    assert_eq!(
        fixture
            .service
            .materialize_image(&digest)
            .await
            .unwrap_err()
            .code(),
        ErrorCode::PlatformUnavailable
    );
    assert_eq!(fixture._artifacts._mock.artifact_gets(), gets);
    assert!(
        fixture
            .service
            .cache
            .acquire_cached(&artifact)
            .await
            .is_err()
    );
    assert_eq!(fixture.protocol.imports.load(Ordering::SeqCst), 0);
    assert!(fixture.protocol.images.lock().await.is_empty());
    assert_eq!(
        fixture
            .service
            .storage
            .admission_snapshot()
            .unwrap()
            .reserved_bytes,
        reserved
    );
}
