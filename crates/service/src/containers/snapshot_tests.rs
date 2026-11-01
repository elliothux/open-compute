use super::{snapshots::SnapshotRequest, test_support::snapshots, test_support::*, *};
use open_compute_storage::containers::{CONTAINER_SNAPSHOT_RETENTION_MS, ContainerSnapshotState};
use serde_json::json;
use std::sync::atomic::Ordering;

async fn running(fixture: &Fixture, authority: &Authority) -> ResolvedLease {
    let startup = if authority.app.scheduling_policy
        == open_compute_storage::containers::ContainerSchedulingPolicy::DurableObject
    {
        let repo = ContainerAuthority::new(fixture.service.storage.db());
        let target = repo
            .target(fixture.service.instance_id(), authority.app.id, 1)
            .unwrap();
        let declarations = repo
            .version_declarations(fixture.service.instance_id(), target.spec.worker_version_id)
            .unwrap();
        Some(json!({"image":declarations[0].images["main"]}))
    } else {
        None
    };
    fixture
        .service
        .resolve(
            authority.request(true, startup),
            10,
            "container-unit-generation",
        )
        .await
        .unwrap();
    fixture.running().await;
    let lease = fixture
        .service
        .resolve(
            authority.request(false, None),
            11,
            "container-unit-generation",
        )
        .await
        .unwrap()
        .lease
        .unwrap();
    assert_eq!(lease.state, ContainerLeaseState::Running);
    *fixture.protocol.snapshot_rootfs.lock().await = snapshots::rootfs(b"saved");
    lease
}

fn request(authority: &Authority, lease: &ResolvedLease) -> SnapshotRequest {
    SnapshotRequest {
        source: authority.stop(lease),
        name: Some("checkpoint".to_owned()),
        directory: None,
    }
}

#[tokio::test]
async fn directory_snapshot_capture_persists_path_and_reports_archive_bytes() {
    for durable_object in [false, true] {
        let fixture = fixture().await;
        let image = fixture.admit_image("main").await;
        let authority = if durable_object {
            do_authority(&fixture.service, &fixture._artifacts, image, false)
        } else {
            authority(&fixture.service, &fixture._artifacts, image, false)
        };
        let lease = running(&fixture, &authority).await;
        let directory =
            open_compute_core::containers::ContainerDirectoryPath::parse("/app/data").unwrap();
        let mut request = request(&authority, &lease);
        request.directory = Some(directory.clone());
        let expected = fixture.protocol.snapshot_rootfs.lock().await.len() as u64;
        fixture.protocol.image_size.store(173, Ordering::SeqCst);
        let response = fixture.service.capture_snapshot(request).await.unwrap();
        assert_eq!(response.snapshot.dir, Some(directory));
        assert_eq!(response.snapshot.size, expected);
        assert_ne!(response.snapshot.size, 173);
        let restarted = fixture.restart_service();
        let repo = ContainerAuthority::new(restarted.storage.db());
        let (saved, image) = repo
            .directory_snapshot_for_restore(
                &authority.object,
                response.snapshot.id,
                open_compute_core::wall_time_ms(),
            )
            .unwrap();
        assert_eq!(saved.directory, response.snapshot.dir);
        assert_eq!(saved.image_size, Some(expected));
        assert_eq!(saved.image_sha256, Some(image.manifest_sha256));
        assert!(
            repo.snapshot_for_restore(
                &authority.object,
                saved.id,
                open_compute_core::wall_time_ms()
            )
            .is_err()
        );
        assert_eq!(fixture.protocol.snapshot_requests.lock().await.len(), 1);
    }
}

#[tokio::test]
async fn snapshot_disk_admission_failure_does_not_reserve_or_stop_the_source() {
    let fixture = fixture().await;
    let image = fixture.admit_image("main").await;
    let authority = do_authority(&fixture.service, &fixture._artifacts, image, false);
    let lease = running(&fixture, &authority).await;
    fixture.service.storage.begin_draining();
    assert!(
        fixture
            .service
            .capture_snapshot(request(&authority, &lease))
            .await
            .is_err()
    );
    let repo = ContainerAuthority::new(fixture.service.storage.db());
    assert!(
        repo.snapshots(fixture.service.instance_id())
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        repo.lease(
            fixture.service.instance_id(),
            authority.stop(&lease).identity()
        )
        .unwrap()
        .state,
        ContainerLeaseState::Running
    );
    assert!(fixture.protocol.snapshot_requests.lock().await.is_empty());
    assert!(fixture.protocol.terminated.lock().await.is_empty());
    assert_eq!(
        fixture
            .service
            .storage
            .admission_snapshot()
            .unwrap()
            .reserved_bytes,
        0
    );
}

#[tokio::test]
async fn snapshot_capture_restores_same_application_after_service_restart_and_retires_artifact() {
    let fixture = fixture().await;
    let image = fixture.admit_image("main").await;
    let authority = do_authority(&fixture.service, &fixture._artifacts, image, false);
    let lease = running(&fixture, &authority).await;
    fixture.protocol.image_size.store(173, Ordering::SeqCst);
    fixture.protocol.image_checks.lock().await.clear();
    let response = fixture
        .service
        .capture_snapshot(request(&authority, &lease))
        .await
        .unwrap();
    let repo = ContainerAuthority::new(fixture.service.storage.db());
    let saved = repo
        .snapshots(fixture.service.instance_id())
        .unwrap()
        .pop()
        .unwrap();
    assert_eq!(saved.state, ContainerSnapshotState::Ready);
    assert_eq!(saved.name.as_deref(), Some("checkpoint"));
    assert_eq!(saved.id, response.snapshot.id);
    assert_eq!(saved.image_size, Some(173));
    assert_eq!(response.snapshot.size, 173);
    assert_ne!(saved.artifact_size, saved.image_size);
    let derived = saved.image_sha256.unwrap();
    assert_ne!(derived, image);
    let snapshot_image = repo.image(fixture.service.instance_id(), &derived).unwrap();
    let snapshot_image_id = format!("sha256:{}", hex::encode(snapshot_image.config_sha256));
    let source_image = repo.image(fixture.service.instance_id(), &image).unwrap();
    let source_image_id = format!("sha256:{}", hex::encode(source_image.config_sha256));
    let checks = fixture.protocol.image_checks.lock().await.clone();
    assert!(!checks.is_empty());
    assert!(checks.iter().all(|id| id == &snapshot_image_id));
    assert!(!checks.contains(&source_image_id));
    let imports_after_capture = fixture.protocol.imports.load(Ordering::SeqCst);
    assert!(
        repo.ordinary_image(fixture.service.instance_id(), &derived)
            .is_err()
    );
    let artifact = open_compute_artifacts::ArtifactRef::new(
        1,
        &hex::encode(saved.artifact_sha256.unwrap()),
        saved.artifact_size.unwrap(),
    )
    .unwrap();
    assert!(
        !fixture
            .service
            .artifacts
            .is_absent(&artifact)
            .await
            .unwrap()
    );
    fixture
        .service
        .stop(&authority.stop(&lease), 12)
        .await
        .unwrap();
    let object = insert_object(&fixture.service, authority.object.namespace_resource_id, 8);
    let target = Authority {
        app: authority.app.clone(),
        object: ContainerObjectAuthority {
            object_id: object,
            ..authority.object
        },
    };
    let restarted = Arc::new(fixture.restart_service());
    assert!(
        restarted
            .resolve(
                target.request(
                    true,
                    Some(json!({
                        "image":format!("image@sha256:{}",hex::encode(derived))
                    }))
                ),
                13,
                "container-unit-generation"
            )
            .await
            .is_err()
    );
    let restored = restarted
        .resolve(
            target.request(
                true,
                Some(json!({
                    "containerSnapshotId":saved.id.to_string(),"enableInternet":false
                })),
            ),
            14,
            "container-unit-generation",
        )
        .await
        .unwrap()
        .lease
        .unwrap();
    assert_eq!(restored.image_sha256, hex::encode(derived));
    assert_eq!(restored.inspection.image, "");
    assert_eq!(
        fixture.protocol.imports.load(Ordering::SeqCst),
        imports_after_capture
    );
    assert_eq!(
        repo.snapshots(fixture.service.instance_id()).unwrap()[0].image_size,
        Some(173)
    );
    fixture.running().await;
    let restored = restarted
        .resolve(target.request(false, None), 15, "container-unit-generation")
        .await
        .unwrap()
        .lease
        .unwrap();
    let persisted = repo.existing_lease(&target.object).unwrap().unwrap();
    assert_eq!(persisted.startup.snapshot_id, Some(saved.id));
    assert_eq!(persisted.state, ContainerLeaseState::Running);
    restarted.stop(&target.stop(&restored), 16).await.unwrap();
    repo.retire_snapshots(
        fixture.service.instance_id(),
        open_compute_core::wall_time_ms() + CONTAINER_SNAPSHOT_RETENTION_MS + 1,
    )
    .unwrap();
    restarted.reconcile_snapshots().await.unwrap();
    assert!(
        repo.snapshots(fixture.service.instance_id())
            .unwrap()
            .is_empty()
    );
    assert!(restarted.artifacts.is_absent(&artifact).await.unwrap());
}

#[tokio::test]
async fn snapshot_image_import_failure_never_publishes_ready_or_releases_unknown_put_charge() {
    for post_load_absent in [false, true] {
        let fixture = fixture().await;
        let image = fixture.admit_image("main").await;
        let authority = do_authority(&fixture.service, &fixture._artifacts, image, false);
        let lease = running(&fixture, &authority).await;
        let imports = fixture.protocol.imports.load(Ordering::SeqCst);
        fixture
            .protocol
            .import_fail
            .store(!post_load_absent, Ordering::SeqCst);
        fixture
            .protocol
            .import_proof_absent
            .store(post_load_absent, Ordering::SeqCst);
        fixture.protocol.image_checks.lock().await.clear();
        assert!(
            fixture
                .service
                .capture_snapshot(request(&authority, &lease))
                .await
                .is_err()
        );
        assert_eq!(fixture.protocol.imports.load(Ordering::SeqCst), imports + 1);
        let repo = ContainerAuthority::new(fixture.service.storage.db());
        let saved = repo
            .snapshots(fixture.service.instance_id())
            .unwrap()
            .pop()
            .unwrap();
        assert_eq!(saved.state, ContainerSnapshotState::Pending);
        assert!(saved.put_attempted);
        assert!(saved.artifact_size.is_some());
        assert!(saved.reserved_bytes > 0);
        assert_eq!(saved.image_size, None);
        assert_eq!(saved.captured_at_ms, None);
        let derived = saved.image_sha256.unwrap();
        assert_ne!(derived, image);
        let snapshot_image = repo.image(fixture.service.instance_id(), &derived).unwrap();
        let snapshot_image_id = format!("sha256:{}", hex::encode(snapshot_image.config_sha256));
        let source_image = repo.image(fixture.service.instance_id(), &image).unwrap();
        let source_image_id = format!("sha256:{}", hex::encode(source_image.config_sha256));
        let checks = fixture.protocol.image_checks.lock().await.clone();
        assert_eq!(checks.len(), if post_load_absent { 2 } else { 1 });
        assert!(checks.iter().all(|id| id == &snapshot_image_id));
        assert!(!checks.contains(&source_image_id));
        let images = fixture.protocol.images.lock().await;
        assert!(images.contains_key(&source_image_id));
        assert!(!images.contains_key(&snapshot_image_id));
        drop(images);
        assert_eq!(
            repo.existing_lease(&authority.object)
                .unwrap()
                .unwrap()
                .state,
            ContainerLeaseState::Running
        );
        assert_eq!(
            fixture
                .service
                .storage
                .admission_snapshot()
                .unwrap()
                .reserved_bytes,
            0
        );

        let restarted = fixture.restart_service();
        restarted.reconcile_snapshots().await.unwrap();
        restarted.reconcile_snapshots().await.unwrap();
        let retained = repo
            .snapshots(fixture.service.instance_id())
            .unwrap()
            .pop()
            .unwrap();
        assert_eq!(retained.state, ContainerSnapshotState::Retiring);
        assert_eq!(retained.image_size, None);
        assert!(retained.put_attempted);
        assert_eq!(retained.reserved_bytes, saved.reserved_bytes);
        assert_eq!(
            repo.lease(fixture.service.instance_id(), saved.source.identity)
                .unwrap()
                .state,
            ContainerLeaseState::Stopped
        );
        assert!(repo.existing_lease(&authority.object).unwrap().is_none());
    }
}

#[tokio::test]
async fn snapshot_authority_rejects_stale_scope_and_nonrunning_sources_before_provider_io() {
    let fixture = fixture().await;
    let image = fixture.admit_image("main").await;
    let authority = do_authority(&fixture.service, &fixture._artifacts, image, false);
    let lease = running(&fixture, &authority).await;
    for change in 0..6 {
        let mut request = request(&authority, &lease);
        match change {
            0 => request.source.opaque_engine_key = "f".repeat(64),
            1 => request.source.route_generation += 1,
            2 => request.source.application_name = "other".into(),
            3 => request.source.lease_generation += 1,
            4 => request.source.instance_id = InstanceId::generate(),
            _ => request.name = Some("x".repeat(257)),
        }
        assert!(fixture.service.capture_snapshot(request).await.is_err());
    }
    let repo = ContainerAuthority::new(fixture.service.storage.db());
    assert!(
        repo.snapshots(fixture.service.instance_id())
            .unwrap()
            .is_empty()
    );
    let persisted = repo.existing_lease(&authority.object).unwrap().unwrap();
    repo.mark_lost(persisted.identity, &persisted.opaque_engine_key, 12)
        .unwrap();
    assert!(
        fixture
            .service
            .capture_snapshot(request(&authority, &lease))
            .await
            .is_err()
    );
    assert!(fixture.protocol.snapshot_requests.lock().await.is_empty());
}

#[tokio::test]
async fn failed_snapshot_keeps_capacity_until_cleanup_recovers_after_service_restart() {
    let fixture = fixture().await;
    let image = fixture.admit_image("main").await;
    let authority = do_authority(&fixture.service, &fixture._artifacts, image, false);
    let lease = running(&fixture, &authority).await;
    fixture.protocol.snapshot_fail.store(true, Ordering::SeqCst);
    fixture
        .protocol
        .snapshot_block
        .store(true, Ordering::SeqCst);
    let service = Arc::clone(&fixture.service);
    let request = request(&authority, &lease);
    let capture = tokio::spawn(async move { service.capture_snapshot(request).await });
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        fixture.protocol.snapshot_entered.notified(),
    )
    .await
    .unwrap();
    assert!(!capture.is_finished());
    assert_eq!(fixture.service.snapshot_captures.lock().unwrap().len(), 1);
    assert!(
        fixture
            .service
            .storage
            .admission_snapshot()
            .unwrap()
            .reserved_bytes
            > 0
    );
    fixture.protocol.snapshot_release.notify_one();
    assert!(capture.await.unwrap().is_err());
    assert!(fixture.service.snapshot_captures.lock().unwrap().is_empty());
    let repo = ContainerAuthority::new(fixture.service.storage.db());
    let saved = repo
        .snapshots(fixture.service.instance_id())
        .unwrap()
        .pop()
        .unwrap();
    assert_eq!(saved.state, ContainerSnapshotState::Pending);
    assert_eq!(
        saved.reserved_bytes,
        fixture.service.provider.max_image_bytes()
    );
    fixture
        .protocol
        .terminate_fail
        .store(true, Ordering::SeqCst);
    let restarted = fixture.restart_service();
    assert!(restarted.reconcile_snapshots().await.is_err());
    let retained = repo
        .snapshots(fixture.service.instance_id())
        .unwrap()
        .pop()
        .unwrap();
    assert_eq!(retained.state, ContainerSnapshotState::Retiring);
    assert_eq!(retained.reserved_bytes, saved.reserved_bytes);
    fixture
        .protocol
        .terminate_fail
        .store(false, Ordering::SeqCst);
    restarted.reconcile_snapshots().await.unwrap();
    assert!(
        repo.snapshots(fixture.service.instance_id())
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        repo.lease(fixture.service.instance_id(), saved.source.identity)
            .unwrap()
            .state,
        ContainerLeaseState::Stopped
    );
}

#[tokio::test]
async fn caller_cancellation_does_not_abandon_an_admitted_snapshot_publication() {
    let fixture = fixture().await;
    let image = fixture.admit_image("main").await;
    let authority = do_authority(&fixture.service, &fixture._artifacts, image, false);
    let lease = running(&fixture, &authority).await;
    fixture
        .protocol
        .snapshot_block
        .store(true, Ordering::SeqCst);
    let service = Arc::clone(&fixture.service);
    let request = request(&authority, &lease);
    let caller = tokio::spawn(async move { service.capture_snapshot(request).await });
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        fixture.protocol.snapshot_entered.notified(),
    )
    .await
    .unwrap();
    caller.abort();
    assert!(matches!(caller.await, Err(error) if error.is_cancelled()));
    let service = Arc::clone(&fixture.service);
    let maintenance = tokio::spawn(async move { service.reconcile_snapshots().await });
    fixture.protocol.snapshot_release.notify_one();
    tokio::time::timeout(std::time::Duration::from_secs(5), maintenance)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let snapshots = ContainerAuthority::new(fixture.service.storage.db())
        .snapshots(fixture.service.instance_id())
        .unwrap();
    assert_eq!(snapshots.len(), 1);
    assert_eq!(snapshots[0].state, ContainerSnapshotState::Ready);
    assert!(fixture.service.snapshot_captures.lock().unwrap().is_empty());
    assert!(fixture.protocol.terminated.lock().await.is_empty());
}

#[tokio::test]
async fn malformed_snapshot_rootfs_is_not_published_or_released_before_source_cleanup() {
    let fixture = fixture().await;
    let image = fixture.admit_image("main").await;
    let authority = do_authority(&fixture.service, &fixture._artifacts, image, false);
    let lease = running(&fixture, &authority).await;
    *fixture.protocol.snapshot_rootfs.lock().await = b"not-an-archive".to_vec();
    assert!(
        fixture
            .service
            .capture_snapshot(request(&authority, &lease))
            .await
            .is_err()
    );
    let repo = ContainerAuthority::new(fixture.service.storage.db());
    let saved = repo
        .snapshots(fixture.service.instance_id())
        .unwrap()
        .pop()
        .unwrap();
    assert_eq!(saved.state, ContainerSnapshotState::Pending);
    assert!(saved.artifact_sha256.is_none());
    fixture.service.reconcile_snapshots().await.unwrap();
    assert!(
        repo.snapshots(fixture.service.instance_id())
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn artifact_publication_failure_keeps_known_identity_and_charge_until_positive_absence() {
    use open_compute_artifacts::Fault;
    let fixture = fixture().await;
    let image = fixture.admit_image("main").await;
    let authority = do_authority(&fixture.service, &fixture._artifacts, image, false);
    let lease = running(&fixture, &authority).await;
    // Source preparation is complete before the object backend becomes unavailable.
    let source = fixture.service.admitted_image(&image).unwrap();
    let source_artifact = open_compute_artifacts::ArtifactRef::new(
        1,
        &hex::encode(source.artifact_sha256),
        source.artifact_size,
    )
    .unwrap();
    let pin = fixture
        .service
        .cache
        .acquire(&fixture.service.artifacts, &source_artifact)
        .await
        .unwrap();
    fixture._artifacts._mock.set_fault(Fault::ServerError);
    let failure = fixture
        .service
        .capture_snapshot(request(&authority, &lease))
        .await
        .err()
        .unwrap();
    assert_eq!(failure.code(), ErrorCode::ObjectStorageUnavailable);
    drop(pin);
    let repo = ContainerAuthority::new(fixture.service.storage.db());
    let saved = repo
        .snapshots(fixture.service.instance_id())
        .unwrap()
        .pop()
        .unwrap();
    assert_eq!(saved.state, ContainerSnapshotState::Pending);
    let artifact = open_compute_artifacts::ArtifactRef::new(
        1,
        &hex::encode(saved.artifact_sha256.unwrap()),
        saved.artifact_size.unwrap(),
    )
    .unwrap();
    fixture._artifacts._mock.set_fault(Fault::DeleteFail);
    let restarted = fixture.restart_service();
    assert!(restarted.reconcile_snapshots().await.is_err());
    let retained = repo
        .snapshots(fixture.service.instance_id())
        .unwrap()
        .pop()
        .unwrap();
    assert_eq!(retained.state, ContainerSnapshotState::Retiring);
    assert_eq!(retained.reserved_bytes, saved.reserved_bytes);
    assert_eq!(retained.artifact_sha256, saved.artifact_sha256);
    fixture._artifacts._mock.set_fault(Fault::None);
    restarted.reconcile_snapshots().await.unwrap();
    assert!(
        repo.snapshots(fixture.service.instance_id())
            .unwrap()
            .is_empty()
    );
    assert!(restarted.artifacts.is_absent(&artifact).await.unwrap());
}

#[tokio::test]
async fn snapshot_lost_put_response_retires_artifact_but_keeps_unknown_publication_charge() {
    use open_compute_artifacts::Fault;
    for fault in [Fault::PutResponseLoss, Fault::LatePutResponseLoss] {
        let fixture = fixture().await;
        let image = fixture.admit_image("main").await;
        let authority = do_authority(&fixture.service, &fixture._artifacts, image, false);
        let lease = running(&fixture, &authority).await;
        let source = fixture.service.admitted_image(&image).unwrap();
        let source_artifact = open_compute_artifacts::ArtifactRef::new(
            1,
            &hex::encode(source.artifact_sha256),
            source.artifact_size,
        )
        .unwrap();
        let _pin = fixture
            .service
            .cache
            .acquire(&fixture.service.artifacts, &source_artifact)
            .await
            .unwrap();
        fixture._artifacts._mock.set_fault(fault);
        assert!(
            fixture
                .service
                .capture_snapshot(request(&authority, &lease))
                .await
                .is_err()
        );
        let repo = ContainerAuthority::new(fixture.service.storage.db());
        let saved = repo
            .snapshots(fixture.service.instance_id())
            .unwrap()
            .pop()
            .unwrap();
        assert_eq!(saved.state, ContainerSnapshotState::Pending);
        assert!(saved.put_attempted);
        let artifact = open_compute_artifacts::ArtifactRef::new(
            1,
            &hex::encode(saved.artifact_sha256.unwrap()),
            saved.artifact_size.unwrap(),
        )
        .unwrap();
        assert_eq!(
            fixture
                .service
                .artifacts
                .is_absent(&artifact)
                .await
                .unwrap(),
            fault == Fault::LatePutResponseLoss
        );
        assert!(fixture.protocol.terminated.lock().await.is_empty());
        fixture._artifacts._mock.set_fault(Fault::None);
        fixture
            .restart_service()
            .reconcile_snapshots()
            .await
            .unwrap();
        let retained = repo.snapshots(fixture.service.instance_id()).unwrap();
        assert_eq!(retained.len(), 1);
        assert_eq!(retained[0].state, ContainerSnapshotState::Retiring);
        assert_eq!(retained[0].reserved_bytes, saved.reserved_bytes);
        assert!(retained[0].put_attempted);
        assert!(
            fixture
                .service
                .artifacts
                .is_absent(&artifact)
                .await
                .unwrap()
        );
        if fault == Fault::LatePutResponseLoss {
            fixture._artifacts._mock.wait_for_late_put().await;
            fixture._artifacts._mock.release_late_put();
            fixture._artifacts._mock.wait_for_late_put_commit().await;
            assert!(
                !fixture
                    .service
                    .artifacts
                    .is_absent(&artifact)
                    .await
                    .unwrap()
            );
            fixture
                .restart_service()
                .reconcile_snapshots()
                .await
                .unwrap();
            assert!(
                fixture
                    .service
                    .artifacts
                    .is_absent(&artifact)
                    .await
                    .unwrap()
            );
            let retained = repo.snapshots(fixture.service.instance_id()).unwrap();
            assert_eq!(retained.len(), 1);
            assert_eq!(retained[0].state, ContainerSnapshotState::Retiring);
            assert!(retained[0].put_attempted);
            assert_eq!(retained[0].reserved_bytes, saved.reserved_bytes);
        }
    }
}

#[tokio::test]
async fn snapshot_shared_artifact_retirement_preserves_ready_restore_and_charge() {
    let fixture = fixture().await;
    let image = fixture.admit_image("main").await;
    let authority = do_authority(&fixture.service, &fixture._artifacts, image, false);
    let source = running(&fixture, &authority).await;
    let first = fixture
        .service
        .capture_snapshot(request(&authority, &source))
        .await
        .unwrap()
        .snapshot
        .id;
    let second = fixture
        .service
        .capture_snapshot(request(&authority, &source))
        .await
        .unwrap()
        .snapshot
        .id;
    let repo = ContainerAuthority::new(fixture.service.storage.db());
    let instance = fixture.service.instance_id();
    let saved = repo.snapshots(instance).unwrap();
    let first_snapshot = saved.iter().find(|snapshot| snapshot.id == first).unwrap();
    let second_snapshot = saved.iter().find(|snapshot| snapshot.id == second).unwrap();
    assert_eq!(
        first_snapshot.artifact_sha256,
        second_snapshot.artifact_sha256
    );
    assert_eq!(first_snapshot.artifact_size, second_snapshot.artifact_size);
    let artifact = open_compute_artifacts::ArtifactRef::new(
        1,
        &hex::encode(first_snapshot.artifact_sha256.unwrap()),
        first_snapshot.artifact_size.unwrap(),
    )
    .unwrap();
    fixture
        .service
        .stop(&authority.stop(&source), 12)
        .await
        .unwrap();
    let now = open_compute_core::wall_time_ms();
    fixture
        .service
        .resolve(
            authority.request(
                true,
                Some(json!({"containerSnapshotId":second,"enableInternet":false})),
            ),
            now,
            "container-unit-generation",
        )
        .await
        .unwrap();
    fixture.running().await;
    let restored = fixture
        .service
        .resolve(
            authority.request(false, None),
            now + 1,
            "container-unit-generation",
        )
        .await
        .unwrap()
        .lease
        .unwrap();
    assert_eq!(restored.state, ContainerLeaseState::Running);
    repo.retire_snapshots(
        instance,
        first_snapshot
            .expires_at_ms
            .max(second_snapshot.expires_at_ms)
            + 1,
    )
    .unwrap();
    fixture.service.reconcile_snapshots().await.unwrap();
    let retained = repo.snapshots(instance).unwrap();
    assert_eq!(retained.len(), 2);
    assert_eq!(
        retained
            .iter()
            .find(|snapshot| snapshot.id == first)
            .unwrap()
            .state,
        ContainerSnapshotState::Retiring
    );
    assert_eq!(
        retained
            .iter()
            .find(|snapshot| snapshot.id == second)
            .unwrap()
            .state,
        ContainerSnapshotState::Ready
    );
    assert_eq!(
        retained
            .iter()
            .map(|snapshot| snapshot.reserved_bytes)
            .sum::<u64>(),
        saved
            .iter()
            .map(|snapshot| snapshot.reserved_bytes)
            .sum::<u64>()
    );
    assert!(
        !fixture
            .service
            .artifacts
            .is_absent(&artifact)
            .await
            .unwrap()
    );
    assert!(
        repo.snapshot_for_restore(&authority.object, second, now + 2)
            .is_ok()
    );
    assert!(
        repo.snapshot_for_restore(&authority.object, first, now + 2)
            .is_err()
    );
    assert_eq!(
        *fixture.protocol.terminated.lock().await,
        [source.opaque_engine_key]
    );
    fixture
        .service
        .stop(&authority.stop(&restored), now + 3)
        .await
        .unwrap();
    repo.retire_snapshots(instance, now + CONTAINER_SNAPSHOT_RETENTION_MS + 10)
        .unwrap();
    fixture
        .restart_service()
        .reconcile_snapshots()
        .await
        .unwrap();
    assert!(repo.snapshots(instance).unwrap().is_empty());
    assert!(
        fixture
            .service
            .artifacts
            .is_absent(&artifact)
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn snapshot_detached_capture_cannot_publish_after_application_delete_fence() {
    let fixture = fixture().await;
    let image = fixture.admit_image("main").await;
    let authority = do_authority(&fixture.service, &fixture._artifacts, image, false);
    let source = running(&fixture, &authority).await;
    fixture
        .protocol
        .snapshot_block
        .store(true, Ordering::SeqCst);
    let service = Arc::clone(&fixture.service);
    let capture = request(&authority, &source);
    let caller = tokio::spawn(async move { service.capture_snapshot(capture).await });
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        fixture.protocol.snapshot_entered.notified(),
    )
    .await
    .unwrap();
    caller.abort();
    assert!(matches!(caller.await, Err(error) if error.is_cancelled()));
    let repo = ContainerAuthority::new(fixture.service.storage.db());
    let instance = fixture.service.instance_id();
    let pending = repo.snapshots(instance).unwrap().pop().unwrap();
    assert_eq!(pending.state, ContainerSnapshotState::Pending);
    assert!(pending.artifact_sha256.is_none());
    let service = Arc::clone(&fixture.service);
    let application = authority.app.id;
    let deleting = tokio::spawn(async move { service.delete_application(application).await });
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if repo
                .lease(instance, authority.stop(&source).identity())
                .unwrap()
                .state
                == ContainerLeaseState::Stopped
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(!deleting.is_finished());
    assert!(
        repo.finish_application_delete(instance, application, open_compute_core::wall_time_ms())
            .is_err()
    );
    let retained = repo.snapshots(instance).unwrap().pop().unwrap();
    assert_eq!(retained.id, pending.id);
    assert_eq!(retained.reserved_bytes, pending.reserved_bytes);
    assert_eq!(retained.state, ContainerSnapshotState::Retiring);
    assert!(
        fixture
            .service
            .snapshot_captures
            .lock()
            .unwrap()
            .contains(&pending.id)
    );
    fixture.protocol.snapshot_release.notify_one();
    tokio::time::timeout(std::time::Duration::from_secs(5), deleting)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(repo.snapshots(instance).unwrap().is_empty());
    assert!(
        repo.pending_application_deletions(instance)
            .unwrap()
            .is_empty()
    );
    assert!(
        repo.applications(instance)
            .unwrap()
            .iter()
            .all(|app| app.id != application)
    );
    assert!(fixture.service.snapshot_captures.lock().unwrap().is_empty());
    assert_eq!(
        *fixture.protocol.terminated.lock().await,
        [source.opaque_engine_key]
    );
}

#[tokio::test]
async fn default_scheduling_rejects_snapshot_capture_and_restore_before_provider_io() {
    let fixture = fixture().await;
    let image = fixture.admit_image("default").await;
    let authority = authority(&fixture.service, &fixture._artifacts, image, false);
    let lease = running(&fixture, &authority).await;
    assert!(
        fixture
            .service
            .capture_snapshot(request(&authority, &lease))
            .await
            .is_err()
    );
    assert!(fixture.protocol.snapshot_requests.lock().await.is_empty());
    let repo = ContainerAuthority::new(fixture.service.storage.db());
    assert!(
        repo.snapshots(fixture.service.instance_id())
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        fixture
            .service
            .storage
            .admission_snapshot()
            .unwrap()
            .reserved_bytes,
        0
    );
    let another = Authority {
        app: authority.app.clone(),
        object: ContainerObjectAuthority {
            object_id: insert_object(&fixture.service, authority.object.namespace_resource_id, 9),
            ..authority.object
        },
    };
    assert!(
        fixture
            .service
            .resolve(
                another.request(
                    true,
                    Some(json!({"containerSnapshotId":ResourceId::generate()}))
                ),
                12,
                "container-unit-generation"
            )
            .await
            .is_err()
    );
    assert!(repo.existing_lease(&another.object).unwrap().is_none());
    assert_eq!(fixture.protocol.registrations.lock().await.len(), 1);
    assert_eq!(
        repo.existing_lease(&authority.object)
            .unwrap()
            .unwrap()
            .state,
        ContainerLeaseState::Running
    );
    assert!(fixture.protocol.terminated.lock().await.is_empty());
}

#[tokio::test]
async fn directory_restore_start_streams_frozen_archive_and_replay_preserves_live_contents() {
    for durable_object in [false, true] {
        let fixture = fixture().await;
        let image = fixture.admit_image("main").await;
        let source = if durable_object {
            do_authority(&fixture.service, &fixture._artifacts, image, false)
        } else {
            authority(&fixture.service, &fixture._artifacts, image, false)
        };
        let lease = running(&fixture, &source).await;
        let original = fixture.protocol.snapshot_rootfs.lock().await.clone();
        let mut capture = request(&source, &lease);
        capture.directory = Some(
            open_compute_core::containers::ContainerDirectoryPath::parse("/app/data").unwrap(),
        );
        let saved = fixture
            .service
            .capture_snapshot(capture)
            .await
            .unwrap()
            .snapshot;
        fixture
            .service
            .stop(&source.stop(&lease), 12)
            .await
            .unwrap();
        let target = Authority {
            app: source.app.clone(),
            object: ContainerObjectAuthority {
                object_id: insert_object(&fixture.service, source.object.namespace_resource_id, 8),
                ..source.object
            },
        };
        let mut startup = json!({"directorySnapshots":[
            {"snapshotId":saved.id.to_string(),"mountPoint":"/restored"},
            {"snapshotId":null,"mountPoint":"/empty"}
        ]});
        if durable_object {
            let repo = ContainerAuthority::new(fixture.service.storage.db());
            let target = repo
                .target(fixture.service.instance_id(), source.app.id, 1)
                .unwrap();
            let declarations = repo
                .version_declarations(fixture.service.instance_id(), target.spec.worker_version_id)
                .unwrap();
            startup["image"] = json!(declarations[0].images["main"]);
        }
        let restarted = fixture.restart_service();
        let restored = restarted
            .resolve(
                target.request(true, Some(startup.clone())),
                13,
                "container-unit-generation",
            )
            .await
            .unwrap()
            .lease
            .unwrap();
        assert_eq!(restored.image_sha256, hex::encode(image));
        let uploads = fixture.protocol.directory_archives.lock().await.clone();
        assert_eq!(
            uploads,
            vec![(restored.opaque_engine_key.clone(), 0, original)]
        );
        let registration = fixture
            .protocol
            .registrations
            .lock()
            .await
            .last()
            .unwrap()
            .clone();
        assert_eq!(registration.directory_snapshots.len(), 2);
        assert!(registration.directory_snapshots[1].archive.is_none());
        fixture.running().await;
        let current = restarted
            .resolve(target.request(false, None), 14, "container-unit-generation")
            .await
            .unwrap()
            .lease
            .unwrap();
        assert_eq!(current.state, ContainerLeaseState::Running);
        let again = fixture
            .restart_service()
            .resolve(
                target.request(true, Some(startup)),
                15,
                "container-unit-generation",
            )
            .await
            .unwrap()
            .lease
            .unwrap();
        assert_eq!(again.opaque_engine_key, restored.opaque_engine_key);
        assert_eq!(*fixture.protocol.directory_archives.lock().await, uploads);
        for plan in [
            json!([{"snapshotId":null,"mountPoint":"/"}]),
            json!([{"snapshotId":null,"mountPoint":"/.open-compute-exec/state"}]),
            json!([{"snapshotId":null,"mountPoint":"/same"},{"snapshotId":null,"mountPoint":"/same"}]),
            json!([]),
        ] {
            assert!(
                restarted
                    .resolve(
                        target.request(true, Some(json!({"directorySnapshots":plan}))),
                        16,
                        "container-unit-generation"
                    )
                    .await
                    .is_err()
            );
        }
        assert_eq!(*fixture.protocol.directory_archives.lock().await, uploads);
        restarted.stop(&target.stop(&restored), 17).await.unwrap();
        let cleanup = fixture
            .protocol
            .cleanup_registrations
            .lock()
            .await
            .last()
            .unwrap()
            .clone();
        assert_eq!(
            cleanup.directory_snapshots,
            registration.directory_snapshots
        );
    }
}
