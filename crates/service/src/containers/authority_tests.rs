use super::{test_support::*, *};
use serde_json::json;
use std::sync::atomic::Ordering;

#[tokio::test]
async fn authority_read_does_not_reserve_and_stale_facets_cannot_start() {
    let fixture = fixture().await;
    let image = fixture.admit_image("main").await;
    let authority = authority(&fixture.service, &fixture._artifacts, image, true);

    assert!(
        fixture
            .service
            .resolve(
                authority.request(false, None),
                10,
                "container-unit-generation"
            )
            .await
            .unwrap()
            .lease
            .is_none()
    );
    for field in 0..6 {
        let mut request = authority.request(true, None);
        match field {
            0 => request.instance_id = InstanceId::generate(),
            1 => request.route_generation += 1,
            2 => request.object_generation += 1,
            3 => request.descriptor_sha256 = "0".repeat(64),
            4 => request.descriptor_sha256 = "A".repeat(64),
            _ => request.application_name = "undeclared".into(),
        }
        assert!(
            fixture
                .service
                .resolve(request, 11, "container-unit-generation")
                .await
                .is_err()
        );
    }

    fixture
        .service
        .delete_application(authority.app.id)
        .await
        .unwrap();

    assert!(
        fixture
            .service
            .resolve(
                authority.request(false, None),
                13,
                "container-unit-generation"
            )
            .await
            .unwrap()
            .lease
            .is_none()
    );
    assert!(
        fixture
            .service
            .resolve(
                authority.request(true, None),
                13,
                "container-unit-generation"
            )
            .await
            .is_err()
    );
}

#[tokio::test]
async fn poststart_requires_exact_running_evidence_and_unknown_state_keeps_same_capacity() {
    let fixture = fixture().await;
    let image = fixture.admit_image("main").await;
    let authority = authority(&fixture.service, &fixture._artifacts, image, true);

    let initial = fixture
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
    assert_eq!(initial.state, ContainerLeaseState::Starting);
    assert!(initial.start_identity.is_none());
    fixture.running().await;
    let running = fixture
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
    assert_eq!(running.state, ContainerLeaseState::Running);
    assert_eq!(
        running.start_identity.as_deref(),
        Some("c".repeat(64).as_str())
    );
    let valid = fixture.protocol.observation.lock().await.clone().unwrap();
    for mutation in 0..6 {
        let mut invalid = valid.clone();
        match mutation {
            0 => invalid.running = false,
            1 => invalid.image = format!("sha256:{}", "0".repeat(64)),
            2 => invalid.manifest_sha256 = "0".repeat(64),
            3 => invalid.provider_contract_sha256 = "0".repeat(64),
            4 => invalid.start_identity = "d".repeat(64),
            _ => invalid.start_identity.clear(),
        }
        *fixture.protocol.observation.lock().await = Some(invalid);
        let response = fixture
            .service
            .resolve(
                authority.request(false, None),
                12,
                "container-unit-generation",
            )
            .await;
        if mutation >= 4 {
            assert!(response.is_err());
        } else {
            let uncertain = response.unwrap().lease.unwrap();
            assert_eq!(uncertain.state, ContainerLeaseState::Lost);
            assert_eq!(uncertain.opaque_engine_key, initial.opaque_engine_key);
        }
    }
    *fixture.protocol.observation.lock().await = None;
    assert_eq!(
        fixture
            .service
            .resolve(
                authority.request(false, None),
                13,
                "container-unit-generation"
            )
            .await
            .unwrap()
            .lease
            .unwrap()
            .state,
        ContainerLeaseState::Lost
    );
    let retried = fixture
        .restart_service()
        .resolve(
            authority.request(true, None),
            14,
            "container-unit-generation",
        )
        .await
        .unwrap()
        .lease
        .unwrap();
    assert_eq!(retried.opaque_engine_key, initial.opaque_engine_key);
    assert_eq!(retried.lease_generation, initial.lease_generation);
    let second = insert_object(&fixture.service, authority.object.namespace_resource_id, 8);
    let mut competing = authority.request(true, None);
    competing.object_id = second;
    assert!(
        fixture
            .service
            .resolve(competing, 15, "container-unit-generation")
            .await
            .is_err()
    );
    assert_eq!(
        ContainerAuthority::new(fixture.service.storage.db())
            .application_leases(fixture.service.instance_id(), authority.app.id)
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn failed_registration_is_durable_lost_and_recovery_does_not_create_replacement() {
    for durable_object in [false, true] {
        let fixture = fixture().await;
        let image = fixture.admit_image("main").await;
        let authority = if durable_object {
            do_authority(&fixture.service, &fixture._artifacts, image, true)
        } else {
            authority(&fixture.service, &fixture._artifacts, image, true)
        };

        let mut startup = json!({"enableInternet":false});
        if durable_object {
            startup["image"] = json!(format!("app@sha256:{}", hex::encode(image)));
        }
        fixture.protocol.register_fail.store(true, Ordering::SeqCst);
        assert!(
            fixture
                .service
                .resolve(
                    authority.request(true, Some(startup)),
                    10,
                    "container-unit-generation"
                )
                .await
                .is_err()
        );
        let repo = ContainerAuthority::new(fixture.service.storage.db());
        let lost = repo.existing_lease(&authority.object).unwrap().unwrap();
        assert_eq!(lost.state, ContainerLeaseState::Lost);
        fixture
            .protocol
            .register_fail
            .store(false, Ordering::SeqCst);
        let recovered = fixture
            .restart_service()
            .resolve(
                authority.request(true, None),
                11,
                "container-unit-generation",
            )
            .await
            .unwrap()
            .lease
            .unwrap();
        assert_eq!(recovered.opaque_engine_key, lost.opaque_engine_key);
        assert_eq!(recovered.state, ContainerLeaseState::Lost);
        assert!(!recovered.enable_internet);
        let identities = fixture.protocol.registrations.lock().await;
        assert_eq!(identities.len(), 2);
        assert_eq!(identities[0].lease_key, identities[1].lease_key);
        assert_eq!(identities[0].startup, identities[1].startup);
        assert_eq!(
            identities[0].scheduling_policy,
            authority.app.scheduling_policy
        );
        assert_eq!(
            identities[0].scheduling_policy,
            identities[1].scheduling_policy
        );
    }
}

#[tokio::test]
async fn stop_is_fenced_by_full_facet_and_requires_positive_provider_cleanup() {
    let fixture = fixture().await;
    let image = fixture.admit_image("main").await;
    let authority = authority(&fixture.service, &fixture._artifacts, image, true);

    let initial = fixture
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
    for mutation in 0..8 {
        let mut stop = authority.stop(&lease);
        match mutation {
            0 => stop.opaque_engine_key = "0".repeat(64),
            1 => stop.lease_generation += 1,
            2 => stop.object_generation += 1,
            3 => stop.route_generation += 1,
            4 => stop.descriptor_sha256 = "0".repeat(64),
            5 => stop.application_name = "other".into(),
            6 => stop.instance_id = InstanceId::generate(),
            _ => stop.descriptor_sha256 = "invalid".into(),
        }
        assert!(fixture.service.stop(&stop, 12).await.is_err());
    }
    assert!(fixture.protocol.terminated.lock().await.is_empty());
    fixture
        .service
        .begin_stop(&authority.stop(&lease), 13)
        .unwrap();
    assert!(
        fixture
            .service
            .resolve(
                authority.request(false, None),
                14,
                "container-unit-generation"
            )
            .await
            .is_err()
    );
    let credential_requests = fixture.credentials.observed.lock().await.len();
    fixture
        .protocol
        .terminate_fail
        .store(true, Ordering::SeqCst);
    let mut for_stop = authority.request(false, None);
    for_stop.for_stop = true;
    assert!(
        fixture
            .service
            .resolve(for_stop, 15, "container-unit-generation")
            .await
            .is_err()
    );
    assert_eq!(
        fixture.credentials.observed.lock().await.len(),
        credential_requests
    );
    let persisted = ContainerAuthority::new(fixture.service.storage.db())
        .existing_lease(&authority.object)
        .unwrap()
        .unwrap();
    assert_eq!(persisted.state, ContainerLeaseState::Lost);
    assert_eq!(persisted.stop_requested_at_ms, Some(13));
    fixture
        .protocol
        .terminate_fail
        .store(false, Ordering::SeqCst);
    let mut for_stop = authority.request(false, None);
    for_stop.for_stop = true;
    assert!(
        fixture
            .restart_service()
            .resolve(for_stop, 16, "container-unit-generation")
            .await
            .unwrap()
            .lease
            .is_none()
    );
    assert_eq!(
        fixture.credentials.observed.lock().await.len(),
        credential_requests
    );
    let history = ContainerAuthority::new(fixture.service.storage.db())
        .application_leases(fixture.service.instance_id(), authority.app.id)
        .unwrap();
    assert_eq!(history[0].state, ContainerLeaseState::Stopped);
    assert!(
        fixture
            .service
            .resolve(
                authority.request(false, None),
                17,
                "container-unit-generation"
            )
            .await
            .unwrap()
            .lease
            .is_none()
    );
    let replacement = fixture
        .service
        .resolve(
            authority.request(true, None),
            18,
            "container-unit-generation",
        )
        .await
        .unwrap()
        .lease
        .unwrap();
    assert_ne!(replacement.opaque_engine_key, initial.opaque_engine_key);
    assert_eq!(replacement.lease_generation, initial.lease_generation + 1);
    assert!(
        fixture
            .service
            .stop(&authority.stop(&lease), 19)
            .await
            .is_err()
    );
    assert_eq!(
        ContainerAuthority::new(fixture.service.storage.db())
            .existing_lease(&authority.object)
            .unwrap()
            .unwrap()
            .opaque_engine_key,
        replacement.opaque_engine_key
    );
}

#[tokio::test]
async fn monitor_purpose_preserves_exited_lease_for_native_wait_without_authorizing_other_operations()
 {
    let fixture = fixture().await;
    let image = fixture.admit_image("main").await;
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
    let mut monitoring = authority.request(false, None);
    monitoring.for_monitor = true;
    assert_eq!(
        fixture
            .service
            .resolve(monitoring, 11, "container-unit-generation")
            .await
            .unwrap()
            .lease
            .unwrap()
            .state,
        ContainerLeaseState::Starting
    );
    fixture.running().await;
    let proved = fixture
        .service
        .resolve(
            authority.request(false, None),
            12,
            "container-unit-generation",
        )
        .await
        .unwrap()
        .lease
        .unwrap();
    assert_eq!(proved.state, ContainerLeaseState::Running);
    fixture
        .protocol
        .observation
        .lock()
        .await
        .as_mut()
        .unwrap()
        .running = false;
    let mut monitoring = authority.request(false, None);
    monitoring.for_monitor = true;
    let exited = fixture
        .service
        .resolve(monitoring, 13, "container-unit-generation")
        .await
        .unwrap()
        .lease
        .unwrap();
    assert_eq!(exited.state, ContainerLeaseState::Running);
    assert_eq!(exited.opaque_engine_key, lease.opaque_engine_key);
    assert_eq!(
        fixture
            .service
            .resolve(
                authority.request(false, None),
                14,
                "container-unit-generation"
            )
            .await
            .unwrap()
            .lease
            .unwrap()
            .state,
        ContainerLeaseState::Lost
    );
    let mut monitoring = authority.request(false, None);
    monitoring.for_monitor = true;
    assert_eq!(
        fixture
            .service
            .resolve(monitoring, 15, "container-unit-generation")
            .await
            .unwrap()
            .lease
            .unwrap()
            .state,
        ContainerLeaseState::Lost
    );
    for mutation in 0..4 {
        let mut monitoring = authority.request(false, None);
        monitoring.for_monitor = true;
        match mutation {
            0 => monitoring.start = true,
            1 => monitoring.for_stop = true,
            2 => monitoring.startup = Some(serde_json::from_value(json!({})).unwrap()),
            _ => monitoring.route_generation += 1,
        }
        assert!(
            fixture
                .service
                .resolve(monitoring, 16, "container-unit-generation")
                .await
                .is_err()
        );
    }
    fixture
        .service
        .begin_stop(&authority.stop(&lease), 17)
        .unwrap();
    let mut monitoring = authority.request(false, None);
    monitoring.for_monitor = true;
    assert!(
        fixture
            .service
            .resolve(monitoring, 18, "container-unit-generation")
            .await
            .is_err()
    );
}
