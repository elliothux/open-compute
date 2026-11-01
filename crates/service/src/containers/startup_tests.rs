use super::{test_support::*, *};
use serde_json::{Value, json};

#[tokio::test]
async fn durable_object_named_custom_profiles_and_explicit_image_are_authoritative() {
    let fixture = fixture().await;
    let image = fixture.admit_image("main").await;
    let authority = do_authority(&fixture.service, &fixture._artifacts, image, true);
    for (startup, cpu, memory, disk, named) in [
        (Some(json!({})), 62_500_000, 256 << 20, 2_000_000_000, None),
        (
            Some(json!({"instanceProfile":"lite"})),
            62_500_000,
            256 << 20,
            2_000_000_000,
            Some("lite"),
        ),
        (
            Some(json!({"resources":{"vcpu":0.125,"memoryMib":512,"diskMb":1000}})),
            125_000_000,
            512 << 20,
            1_000_000_000,
            None,
        ),
    ] {
        let mut startup = startup.unwrap();
        startup["image"] = json!(format!("app@sha256:{}", hex::encode(image)));
        let execution = fixture
            .service
            .startup_execution(
                &authority.request(true, Some(startup)),
                &authority.app,
                &authority.object,
                &"a".repeat(64),
                10,
            )
            .unwrap();
        assert_eq!(execution.resources.cpu_nanos, cpu);
        assert_eq!(execution.resources.memory_bytes, memory);
        assert_eq!(execution.resources.disk_bytes, disk);
        assert_eq!(
            execution
                .startup
                .instance_profile
                .map(open_compute_core::containers::ContainerInstanceProfile::as_str),
            named
        );
        assert_eq!(execution.image_sha256, image);
        assert!(execution.enable_internet);
        let options = fixture
            .service
            .open_start_options(
                &execution.startup_envelope,
                &ContainerLeaseIdentity {
                    namespace_resource_id: authority.object.namespace_resource_id,
                    object_id: authority.object.object_id,
                    object_generation: 1,
                    lease_generation: 1,
                }
                .owner_key(fixture.service.instance_id()),
                &"a".repeat(64),
            )
            .unwrap();
        assert!(options.entrypoint.is_none());
        assert!(options.environment_variables.is_empty());
    }
}

#[tokio::test]
async fn omitted_durable_object_lite_exceeding_policy_never_reserves_or_calls_provider() {
    let fixture = fixture().await;
    let image = fixture.admit_image("small-policy").await;
    let policy = open_compute_core::containers::ContainerResources {
        memory_bytes: 128 << 20,
        ..fixture.service.provider.resources()
    };
    let authority =
        do_authority_with_resources(&fixture.service, &fixture._artifacts, image, false, policy);

    for instance in [json!({}), json!({"instanceProfile":"lite"})] {
        let mut startup = instance;
        startup["image"] = json!(format!("app@sha256:{}", hex::encode(image)));
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
        assert!(
            ContainerAuthority::new(fixture.service.storage.db())
                .existing_lease(&authority.object)
                .unwrap()
                .is_none()
        );
    }
    assert!(fixture.protocol.registrations.lock().await.is_empty());
    assert_eq!(
        fixture
            .protocol
            .imports
            .load(std::sync::atomic::Ordering::SeqCst),
        0
    );
    let execution = fixture
        .service
        .startup_execution(
            &authority.request(
                true,
                Some(json!({
                    "image":format!("app@sha256:{}", hex::encode(image)),
                    "resources":{"vcpu":0.03125,"memoryMib":128,"diskMb":1000}
                })),
            ),
            &authority.app,
            &authority.object,
            &"a".repeat(64),
            11,
        )
        .unwrap();
    assert_eq!(execution.resources.cpu_nanos, 31_250_000);
    assert_eq!(execution.resources.memory_bytes, 128 << 20);
}

#[tokio::test]
async fn invalid_startup_never_reserves_capacity_or_registers_engine_identity() {
    let fixture = fixture().await;
    let image = fixture.admit_image("main").await;
    let authority = do_authority(&fixture.service, &fixture._artifacts, image, false);

    for startup in [
        json!({"image":"registry.example.test/app:latest"}),
        json!({"image":format!("app@sha256:{}","f".repeat(64))}),
        json!({"instanceProfile":"standard-1"}),
        json!({"instanceProfile":"basic"}),
        json!({"instanceProfile":"lite","resources":{"vcpu":0.1,"memoryMib":256,"diskMb":1000}}),
        json!({"resources":{"vcpu":2.0,"memoryMib":256,"diskMb":1000}}),
        json!({"resources":{"vcpu":0.1,"memoryMib":2048,"diskMb":1000}}),
        json!({"resources":{"vcpu":0.1,"memoryMib":256,"diskMb":5000}}),
        json!({"environmentVariables":["TOKEN=first","TOKEN=second"]}),
        json!({"entrypoint":["bad\u{0000}command"]}),
        json!({"labels":[{"name":"dev.open-compute.owner","value":"forged"}]}),
        json!({"enableInternet":true}),
    ] {
        let mut startup = startup;
        if startup.get("image").is_none() {
            startup["image"] = json!(format!("app@sha256:{}", hex::encode(image)));
        }
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
        assert!(
            ContainerAuthority::new(fixture.service.storage.db())
                .existing_lease(&authority.object)
                .unwrap()
                .is_none()
        );
    }
    assert!(fixture.protocol.registrations.lock().await.is_empty());
}

#[tokio::test]
async fn startup_ciphertext_binds_owner_key_and_replays_frozen_options_after_service_restart() {
    let fixture = fixture().await;
    let image = fixture.admit_image("main").await;
    let override_image = fixture.admit_image("override").await;
    let authority = do_authority(&fixture.service, &fixture._artifacts, image, true);
    let startup = json!({"image":format!("app@sha256:{}",hex::encode(override_image)),"instanceProfile":"lite","enableInternet":false,
        "entrypoint":["/app","serve"],"environmentVariables":["API_TOKEN=unit-secret-value"],"labels":[{"name":"app.mode","value":"unit"}]});

    let first = fixture
        .service
        .resolve(
            authority.request(true, Some(startup.clone())),
            10,
            "container-unit-generation",
        )
        .await
        .unwrap()
        .lease
        .unwrap();
    let persisted = ContainerAuthority::new(fixture.service.storage.db())
        .existing_lease(&authority.object)
        .unwrap()
        .unwrap();
    let ciphertext = &persisted.startup_envelope;
    assert!(!String::from_utf8_lossy(ciphertext).contains("unit-secret-value"));
    let options = fixture.service.start_options(&persisted).unwrap();
    assert_eq!(
        options.environment_variables,
        ["API_TOKEN=unit-secret-value"]
    );
    assert_eq!(
        options.entrypoint,
        Some(vec!["/app".into(), "serve".into()])
    );
    assert!(!options.enable_internet);
    for (owner, key) in [
        (&"0".repeat(64), &persisted.opaque_engine_key),
        (
            &persisted.identity.owner_key(fixture.service.instance_id()),
            &"0".repeat(64),
        ),
    ] {
        assert!(
            fixture
                .service
                .open_start_options(ciphertext, owner, key)
                .is_err()
        );
    }
    let mut corrupted = ciphertext.clone();
    corrupted[0] = b'!';
    assert!(
        fixture
            .service
            .open_start_options(
                &corrupted,
                &persisted.identity.owner_key(fixture.service.instance_id()),
                &persisted.opaque_engine_key
            )
            .is_err()
    );
    let restarted = fixture.restart_service();
    let replay = restarted
        .resolve(
            authority.request(true, None),
            11,
            "container-unit-generation",
        )
        .await
        .unwrap()
        .lease
        .unwrap();
    assert_eq!(replay.opaque_engine_key, first.opaque_engine_key);
    assert_eq!(replay.image_sha256, hex::encode(override_image));
    assert_eq!(replay.resources, first.resources);
    assert_eq!(replay.inspection.image, startup["image"].as_str().unwrap());
    assert_eq!(replay.inspection.labels, first.inspection.labels);
    let native: Value = serde_json::from_str(&replay.native_startup_json).unwrap();
    assert_eq!(
        native["source"]["image"],
        format!("startup@sha256:{}", hex::encode(override_image))
    );
    assert_eq!(native["instance"]["named"], "lite");
    assert_eq!(
        native["environmentVariables"],
        json!(["API_TOKEN=unit-secret-value"])
    );
    for change in [
        json!({"environmentVariables":["API_TOKEN=replaced"]}),
        json!({"enableInternet":true}),
        json!({"entrypoint":["/different"]}),
    ] {
        let mut changed = startup.clone();
        changed
            .as_object_mut()
            .unwrap()
            .extend(change.as_object().unwrap().clone());
        assert!(
            restarted
                .resolve(
                    authority.request(true, Some(changed)),
                    12,
                    "container-unit-generation"
                )
                .await
                .is_err()
        );
    }
    let mut incompatible = persisted.clone();
    incompatible.enable_internet = true;
    assert!(restarted.start_options(&incompatible).is_err());
    incompatible.startup_envelope.clear();
    assert!(restarted.start_options(&incompatible).is_err());
    assert_eq!(
        ContainerAuthority::new(fixture.service.storage.db())
            .application_leases(fixture.service.instance_id(), authority.app.id)
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn corrupt_persisted_startup_rejects_service_recovery_without_releasing_capacity() {
    let fixture = fixture().await;
    let image = fixture.admit_image("startup-integrity").await;
    let authority = authority(&fixture.service, &fixture._artifacts, image, true);
    let key = "a".repeat(64);
    let mut execution = fixture
        .service
        .startup_execution(
            &authority.request(
                true,
                Some(json!({
                    "enableInternet":false,
                    "entrypoint":["/app","serve"],
                    "environmentVariables":["API_TOKEN=unit-startup-integrity"]
                })),
            ),
            &authority.app,
            &authority.object,
            &key,
            10,
        )
        .unwrap();
    let mut envelope: open_compute_storage::crypto::SecretEnvelope =
        serde_json::from_slice(&execution.startup_envelope).unwrap();
    envelope.ciphertext[0] ^= 1;
    execution.startup_envelope = serde_json::to_vec(&envelope).unwrap();
    // Persist the fault through the storage authority without disabling immutable-row guards.
    let repo = ContainerAuthority::new(fixture.service.storage.db());
    let lease = repo
        .reserve(&authority.object, &key, &execution, 4, 10)
        .unwrap();
    let persisted = repo
        .lease(fixture.service.instance_id(), lease.identity)
        .unwrap();
    let restarted = fixture.restart_service();

    for service in [fixture.service.as_ref(), &restarted] {
        assert!(service.start_options(&persisted).is_err());
        assert!(
            service
                .resolve(
                    authority.request(true, None),
                    11,
                    "container-unit-generation"
                )
                .await
                .is_err()
        );
        let retained = ContainerAuthority::new(service.storage.db())
            .existing_lease(&authority.object)
            .unwrap()
            .unwrap();
        assert_eq!(retained, persisted);
    }
    assert!(fixture.protocol.registrations.lock().await.is_empty());
}

#[tokio::test]
async fn default_scheduling_uses_configuration_and_rejects_explicit_image_and_resources() {
    let fixture = fixture().await;
    let image = fixture.admit_image("default-startup").await;
    let authority = authority(&fixture.service, &fixture._artifacts, image, true);
    let default = fixture
        .service
        .startup_execution(
            &authority.request(true, None),
            &authority.app,
            &authority.object,
            &"a".repeat(64),
            10,
        )
        .unwrap();
    assert_eq!(default.image_sha256, image);
    assert_eq!(default.resources, fixture.service.provider.resources());

    for startup in [
        json!({"image":format!("app@sha256:{}",hex::encode(image))}),
        json!({"instanceProfile":"lite"}),
        json!({"resources":{"vcpu":1.0,"memoryMib":1024,"diskMb":4000}}),
    ] {
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
        assert!(
            ContainerAuthority::new(fixture.service.storage.db())
                .existing_lease(&authority.object)
                .unwrap()
                .is_none()
        );
    }
    assert!(fixture.protocol.registrations.lock().await.is_empty());
    assert_eq!(
        fixture
            .protocol
            .imports
            .load(std::sync::atomic::Ordering::SeqCst),
        0
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
}
