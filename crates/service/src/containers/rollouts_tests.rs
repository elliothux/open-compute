use super::{test_support::*, *};
use crate::runtime_bridge::WorkerdTransport;
use axum::{Router, body::to_bytes, extract::Request};
use serde_json::json;
use std::sync::atomic::{AtomicUsize, Ordering};

#[tokio::test]
async fn container_rollout_recovers_unknown_stop_and_starts_frozen_replacement_before_completing() {
    let fixture = Arc::new(fixture().await);
    let image = fixture.admit_image("original").await;
    let authority = Arc::new(authority(
        &fixture.service,
        &fixture._artifacts,
        image,
        true,
    ));

    let initial = fixture
        .service
        .resolve(
            authority.request(
                true,
                Some(json!({"enableInternet":false,"environmentVariables":["MODE=original"]})),
            ),
            10,
            "container-unit-generation",
        )
        .await
        .unwrap()
        .lease
        .unwrap();
    fixture.running().await;
    fixture
        .service
        .resolve(
            authority.request(false, None),
            11,
            "container-unit-generation",
        )
        .await
        .unwrap();
    let repo = ContainerAuthority::new(fixture.service.storage.db());
    let replacement = fixture.admit_image("replacement").await;
    let mut spec = repo
        .target(fixture.service.instance_id(), authority.app.id, 1)
        .unwrap()
        .spec;
    spec.image_sha256 = Some(replacement);
    let (mut configuration, _, _) = fixture
        .service
        .application_configuration(&authority.app)
        .unwrap();
    configuration.image = format!(
        "registry.example.test/{}/authority-app@sha256:{}",
        fixture.service.instance_id(),
        hex::encode(replacement),
    );
    let configuration = serde_json::to_vec(&configuration).unwrap();
    assert_eq!(
        repo.commit_application_configuration(
            fixture.service.instance_id(),
            &authority.app,
            &spec,
            &configuration,
            open_compute_storage::containers::ContainerApplicationMetadataPatch::default(),
            12,
        )
        .unwrap()
        .generation,
        2
    );
    let rollout = ResourceId::generate();
    repo.create_rollout(
        fixture.service.instance_id(),
        authority.app.id,
        &open_compute_storage::containers::ContainerRolloutCreate {
            id: rollout,
            target_configuration: &configuration,
            steps: &[100],
            details: None,
        },
        13,
    )
    .unwrap();
    let no_runtime = WorkerdTransport::for_test_endpoint(std::path::Path::new("/tmp/missing.sock"));
    fixture
        .protocol
        .terminate_fail
        .store(true, Ordering::SeqCst);
    assert!(
        fixture
            .service
            .reconcile_rollouts(&no_runtime)
            .await
            .is_err()
    );
    assert_eq!(*fixture.protocol.stop_grace_millis.lock().await, [0]);
    let old = repo
        .lease(
            fixture.service.instance_id(),
            ContainerLeaseIdentity {
                namespace_resource_id: authority.object.namespace_resource_id,
                object_id: authority.object.object_id,
                object_generation: 1,
                lease_generation: initial.lease_generation,
            },
        )
        .unwrap();
    assert_eq!(old.state, ContainerLeaseState::Lost);
    assert!(old.stop_requested_at_ms.is_some());
    assert_eq!(
        repo.rollout_progress(fixture.service.instance_id(), authority.app.id, rollout)
            .unwrap(),
        (1, 0)
    );
    assert_eq!(fixture.protocol.registrations.lock().await.len(), 1);
    fixture
        .protocol
        .terminate_fail
        .store(false, Ordering::SeqCst);
    let calls = Arc::new(AtomicUsize::new(0));
    let listener_dir = tempfile::tempdir_in("/tmp").unwrap();
    let listener_path = listener_dir.path().join("runtime.sock");
    let listener = tokio::net::UnixListener::bind(&listener_path).unwrap();
    let port = listener_path.clone();
    let fixture_for_start = Arc::clone(&fixture);
    let authority_for_start = Arc::clone(&authority);

    let observed = Arc::clone(&calls);
    let app=Router::new().fallback(move |request:Request| {
        let fixture=Arc::clone(&fixture_for_start);let authority=Arc::clone(&authority_for_start);let calls=Arc::clone(&observed);
        async move {
            assert_eq!(request.uri().path(),"/internal/container-start");

            let bytes=to_bytes(request.into_body(),4096).await.unwrap();
            assert_eq!(serde_json::from_slice::<serde_json::Value>(&bytes).unwrap(),json!({"namespaceResourceId":authority.object.namespace_resource_id,"objectId":authority.object.object_id,"objectGeneration":1}));
            let repo=ContainerAuthority::new(fixture.service.storage.db());
            assert_eq!(repo.application_leases(fixture.service.instance_id(),authority.app.id).unwrap()[0].state,ContainerLeaseState::Stopped);

            fixture.service.resolve(authority.request(true,None),
20,
"container-unit-generation").await.unwrap();
            fixture.running().await;
            fixture.service.resolve(authority.request(false,None),
21,
"container-unit-generation").await.unwrap();
            calls.fetch_add(1,Ordering::SeqCst);
            axum::Json(json!({"started":true}))
        }
    });
    let (shutdown, receiver) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                let _ = receiver.await;
            })
            .await
            .unwrap();
    });
    let transport = WorkerdTransport::for_test_endpoint(&port);
    let restarted = fixture.restart_service();
    restarted.reconcile_rollouts(&transport).await.unwrap();
    assert_eq!(*fixture.protocol.stop_grace_millis.lock().await, [0, 0]);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        repo.rollout(fixture.service.instance_id(), authority.app.id, rollout)
            .unwrap()
            .state,
        "running"
    );
    restarted.reconcile_rollouts(&transport).await.unwrap();
    assert_eq!(
        repo.rollout(fixture.service.instance_id(), authority.app.id, rollout)
            .unwrap()
            .state,
        "completed"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let current = repo.existing_lease(&authority.object).unwrap().unwrap();
    assert_eq!(current.target_generation, 2);
    assert_eq!(current.effective_image_sha256, replacement);
    let mut replacement_startup = old.startup.clone();
    replacement_startup.image_reference = format!(
        "registry.example.test/{}/authority-app@sha256:{}",
        fixture.service.instance_id(),
        hex::encode(replacement)
    );
    assert_eq!(current.startup, replacement_startup);
    assert_eq!(
        restarted
            .start_options(&current)
            .unwrap()
            .environment_variables,
        ["MODE=original"]
    );
    drop(transport);
    shutdown.send(()).unwrap();
    server.await.unwrap();
}

#[tokio::test]
async fn container_maintenance_reclaims_only_expired_owned_spools_and_resumes_application_deletion()
{
    let fixture = fixture().await;
    let instance = fixture.service.instance_id();
    let repo = ContainerAuthority::new(fixture.service.storage.db());
    let expired = ResourceId::generate();
    let orphan = ResourceId::generate();
    let live = ResourceId::generate();
    let repository = format!("{instance}/app");
    repo.create_registry_upload(instance, expired, &repository, 1)
        .unwrap();
    repo.create_registry_upload(instance, live, &repository, i64::MAX)
        .unwrap();
    for id in [expired, orphan, live] {
        std::fs::write(
            fixture.service.registry_upload_path(id).unwrap(),
            b"spooled",
        )
        .unwrap();
    }
    let unrelated = fixture
        .service
        .storage
        .data_dir()
        .prepare_tmp_dir()
        .unwrap()
        .join("another-product.part");
    std::fs::write(&unrelated, b"keep").unwrap();
    let image = fixture.admit_image("original").await;
    let authority = authority(&fixture.service, &fixture._artifacts, image, true);

    let lease = fixture
        .service
        .resolve(
            authority.request(true, None),
            10,
            "container-unit-generation",
        )
        .await
        .unwrap()
        .lease
        .unwrap();
    fixture
        .protocol
        .terminate_fail
        .store(true, Ordering::SeqCst);
    assert!(
        fixture
            .service
            .delete_application(authority.app.id)
            .await
            .is_err()
    );
    assert_eq!(
        repo.application_leases(instance, authority.app.id).unwrap()[0].state,
        ContainerLeaseState::Lost
    );
    assert_eq!(
        repo.pending_application_deletions(instance).unwrap(),
        [authority.app.id]
    );
    fixture
        .protocol
        .terminate_fail
        .store(false, Ordering::SeqCst);
    let transport = WorkerdTransport::for_test_endpoint(std::path::Path::new("/tmp/missing.sock"));
    fixture
        .restart_service()
        .reconcile_rollouts(&transport)
        .await
        .unwrap();
    assert!(
        repo.pending_application_deletions(instance)
            .unwrap()
            .is_empty()
    );
    assert!(repo.applications(instance).unwrap().is_empty());
    assert!(
        !fixture
            .service
            .registry_upload_path(expired)
            .unwrap()
            .exists()
    );
    assert!(
        !fixture
            .service
            .registry_upload_path(orphan)
            .unwrap()
            .exists()
    );
    assert!(fixture.service.registry_upload_path(live).unwrap().exists());
    assert!(unrelated.exists());
    assert!(
        fixture
            .protocol
            .terminated
            .lock()
            .await
            .contains(&lease.opaque_engine_key)
    );
}
