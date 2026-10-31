use super::*;

#[test]
fn effective_startup_is_immutable_scoped_and_restarts_without_plaintext() {
    let fixture = fixture_with_policy(4, true);
    let instance = fixture.storage.identity().instance_id;
    let repo = ContainerAuthority::new(fixture.storage.db());
    let alternate = image(21);
    repo.admit_image(instance, &alternate).unwrap();
    let engine = "8".repeat(64);
    let identity = ContainerLeaseIdentity {
        namespace_resource_id: fixture.authority.namespace_resource_id,
        object_id: fixture.authority.object_id,
        object_generation: fixture.authority.object_generation,
        lease_generation: 1,
    };
    let secret = b"{\"environmentVariables\":[\"TOKEN=never-plaintext-storage\"]}";
    let envelope = fixture
        .storage
        .crypto()
        .seal_container_startup(
            &open_compute_core::SecretBytes::new(secret.to_vec()),
            instance,
            &identity.owner_key(instance),
            &engine,
        )
        .unwrap();
    let mut execution = ContainerExecutionSpec {
        directory_snapshots: Vec::new(),
        image_sha256: alternate.manifest_sha256,
        resources: fixture.spec.resources,
        enable_internet: false,
        startup: ContainerLeaseStartup {
            image_reference: format!(
                "registry/alternate@sha256:{}",
                hex::encode(alternate.manifest_sha256)
            ),
            snapshot_id: None,
            instance_profile: Some(open_compute_core::containers::ContainerInstanceProfile::Lite),
            image_override: true,
            resources_override: true,
        },
        startup_envelope: serde_json::to_vec(&envelope).unwrap(),
    };
    execution.resources.cpu_nanos /= 2;
    let lease = repo
        .reserve(&fixture.authority, &engine, &execution, 10, 30)
        .unwrap();
    assert_eq!(lease.effective_image_sha256, alternate.manifest_sha256);
    assert_eq!(lease.effective_resources, execution.resources);
    assert_eq!(
        lease.startup.instance_profile,
        Some(open_compute_core::containers::ContainerInstanceProfile::Lite)
    );
    assert!(!lease.enable_internet);
    let lease_debug = format!("{lease:?}");
    assert!(!lease_debug.contains(&engine));
    assert!(!lease_debug.contains("startup_envelope"));
    assert!(!format!("{execution:?}").contains("startup_envelope"));
    assert_eq!(
        repo.reserve(&fixture.authority, &"9".repeat(64), &execution, 10, 31)
            .unwrap(),
        lease
    );
    let mut changed = execution.clone();
    changed.resources.memory_bytes /= 2;
    assert!(
        repo.reserve(&fixture.authority, &engine, &changed, 10, 32)
            .is_err()
    );
    changed = execution.clone();
    changed.image_sha256 = fixture.spec.image_sha256.unwrap();
    assert!(
        repo.reserve(&fixture.authority, &engine, &changed, 10, 33)
            .is_err()
    );
    changed = execution.clone();
    changed.startup.instance_profile =
        Some(open_compute_core::containers::ContainerInstanceProfile::Basic);
    assert!(
        repo.reserve(&fixture.authority, &engine, &changed, 10, 34)
            .is_err()
    );
    repo.mark_starting(lease.identity, &engine, 35).unwrap();
    let running = ContainerRunningObservation {
        opaque_engine_key: &engine,
        start_identity: &"a".repeat(64),
        image_sha256: &alternate.manifest_sha256,
        provider_contract_sha256: &fixture.spec.provider_contract_sha256,
    };
    repo.observe_running(lease.identity, &running, 36).unwrap();
    let incorrect = ContainerRunningObservation {
        image_sha256: fixture.spec.image_sha256.as_ref().unwrap(),
        ..running
    };
    assert!(
        repo.observe_running(lease.identity, &incorrect, 37)
            .is_err()
    );
    fixture.storage.db().with_read(|conn| {
        let bytes:Vec<u8>=conn.query_row("SELECT startup_envelope_json FROM container_instance_leases WHERE opaque_engine_key=?1",[&engine],|r|r.get(0)).unwrap();
        assert!(!String::from_utf8_lossy(&bytes).contains("never-plaintext-storage"));
        Ok(())
    }).unwrap();
    let root = fixture.directory.path().join("data");
    let authority = fixture.authority;
    drop(fixture.storage);
    let storage = PlatformStorage::bootstrap(&configuration(&root), &SystemClock).unwrap();
    let recovered = ContainerAuthority::new(storage.db())
        .lease(instance, lease.identity)
        .unwrap();
    assert_eq!(recovered.effective_resources, execution.resources);
    assert_eq!(
        recovered.startup.image_reference,
        execution.startup.image_reference
    );
    let recovered_envelope: crate::crypto::SecretEnvelope =
        serde_json::from_slice(&recovered.startup_envelope).unwrap();
    assert_eq!(
        storage
            .crypto()
            .open_container_startup(
                &recovered_envelope,
                instance,
                &identity.owner_key(instance),
                &engine
            )
            .unwrap()
            .expose(),
        secret
    );
    assert!(
        storage
            .crypto()
            .open_container_startup(
                &recovered_envelope,
                instance,
                &identity.owner_key(instance),
                &"f".repeat(64)
            )
            .is_err()
    );
    let repo = ContainerAuthority::new(storage.db());
    repo.begin_stop(lease.identity, &engine, 40).unwrap();
    repo.finish_stopped(
        lease.identity,
        &ContainerTermination {
            opaque_engine_key: &engine,
            start_identity: Some(running.start_identity),
            terminated: true,
            cleanup_complete: true,
            late_operations_fenced: true,
        },
        41,
    )
    .unwrap();
    let mut missing = execution.clone();
    missing.image_sha256 = [99; 32];
    assert!(
        repo.reserve(&authority, &"9".repeat(64), &missing, 10, 42)
            .is_err()
    );
    missing = execution.clone();
    missing.resources.cpu_nanos = fixture.spec.resources.cpu_nanos + 1;
    assert!(
        repo.reserve(&authority, &"9".repeat(64), &missing, 10, 43)
            .is_err()
    );
    assert!(
        serde_json::from_slice::<ContainerLeaseStartup>(
            br#"{"snapshotId":null,"imageReference":"registry/app@sha256:0000000000000000000000000000000000000000000000000000000000000000","instanceProfile":"unknown","imageOverride":false,"resourcesOverride":false}"#
        )
        .is_err()
    );
    let envelope = storage
        .crypto()
        .seal_container_startup(
            &open_compute_core::SecretBytes::new(secret.to_vec()),
            instance,
            &identity.owner_key(instance),
            &"9".repeat(64),
        )
        .unwrap();
    execution.startup_envelope = serde_json::to_vec(&envelope).unwrap();
    let new = repo
        .reserve(&authority, &"9".repeat(64), &execution, 10, 45)
        .unwrap();
    assert_eq!(
        new.identity.owner_key(instance),
        identity.owner_key(instance)
    );
    assert_eq!(new.identity.lease_generation, 2);
}

#[test]
fn startup_envelope_is_required_by_sqlite_and_cannot_be_empty() {
    let fixture = fixture(1);
    let repo = ContainerAuthority::new(fixture.storage.db());
    let key = "a".repeat(64);
    let mut spec = execution(&fixture.storage, &fixture.authority, &fixture.spec, &key);
    spec.startup_envelope.clear();
    assert!(
        repo.reserve(&fixture.authority, &key, &spec, 10, 30)
            .is_err()
    );
    let spec = execution(&fixture.storage, &fixture.authority, &fixture.spec, &key);
    let authority = &fixture.authority;
    fixture.storage.db().with_immediate(|tx| {
        let result = tx.execute(
            "INSERT INTO container_instance_leases
             (application_id,namespace_resource_id,object_id,object_generation,lease_generation,
              target_generation,opaque_engine_key,state,created_at_ms,updated_at_ms,
              effective_image_sha256,effective_resources_json,enable_internet,startup_json,startup_envelope_json)
             VALUES (?1,?2,?3,?4,1,1,?5,'admitting',30,30,?6,?7,?8,?9,NULL)",
            params![authority.application_id,authority.namespace_resource_id,
                authority.object_id.to_string(),authority.object_generation,key,spec.image_sha256.as_slice(),
                serde_json::to_vec(&spec.resources).unwrap(),spec.enable_internet,
                serde_json::to_vec(&spec.startup).unwrap()],
        );
        assert!(matches!(result, Err(rusqlite::Error::SqliteFailure(error, _))
            if error.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_NOTNULL));
        Ok(())
    }).unwrap();
    assert!(
        repo.application_leases(authority.instance_id, authority.application_id)
            .unwrap()
            .is_empty()
    );
    let lease = repo.reserve(authority, &key, &spec, 10, 31).unwrap();
    assert_eq!(lease.startup_envelope, spec.startup_envelope);
}

#[test]
fn public_image_reference_is_required_digest_bound_and_immutable() {
    let fixture = fixture(2);
    let repo = ContainerAuthority::new(fixture.storage.db());
    let key = "a".repeat(64);
    let mut spec = execution(&fixture.storage, &fixture.authority, &fixture.spec, &key);
    let digest = hex::encode(spec.image_sha256);
    for invalid in [
        String::new(),
        format!("registry/app:{digest}"),
        format!("registry/app@sha256:{}", "f".repeat(64)),
        format!("reg istry/app@sha256:{digest}"),
        format!("registry/é@sha256:{digest}"),
        format!("{}@sha256:{digest}", "x".repeat(513)),
        format!("registry/app@sha256:{}", "A".repeat(64)),
    ] {
        spec.startup.image_reference = invalid;
        assert!(
            repo.reserve(&fixture.authority, &key, &spec, 10, 30)
                .is_err()
        );
        assert!(
            repo.application_leases(
                fixture.authority.instance_id,
                fixture.authority.application_id
            )
            .unwrap()
            .is_empty()
        );
    }
    spec.startup.image_reference = format!("{}@sha256:{digest}", "x".repeat(512));
    let lease = repo
        .reserve(&fixture.authority, &key, &spec, 10, 31)
        .unwrap();
    assert_eq!(lease.startup.image_reference, spec.startup.image_reference);
    spec.startup.image_reference = format!("another/app@sha256:{digest}");
    assert!(
        repo.reserve(&fixture.authority, &key, &spec, 10, 32)
            .is_err()
    );
    let mut missing = serde_json::to_value(&lease.startup).unwrap();
    missing.as_object_mut().unwrap().remove("imageReference");
    assert!(serde_json::from_value::<ContainerLeaseStartup>(missing).is_err());
}

#[test]
fn persisted_public_image_reference_is_validated_before_exposing_a_lease() {
    for missing in [false, true] {
        let fixture = fixture(1);
        let repo = ContainerAuthority::new(fixture.storage.db());
        let key = "a".repeat(64);
        let spec = execution(&fixture.storage, &fixture.authority, &fixture.spec, &key);
        let mut startup = serde_json::to_value(&spec.startup).unwrap();
        if missing {
            startup.as_object_mut().unwrap().remove("imageReference");
        } else {
            startup["imageReference"] = serde_json::json!("registry/app:mutable");
        }
        let authority = &fixture.authority;
        fixture.storage.db().with_immediate(|tx| {
            tx.execute("INSERT INTO container_instance_leases
                (application_id,namespace_resource_id,object_id,object_generation,lease_generation,
                target_generation,opaque_engine_key,state,created_at_ms,updated_at_ms,
                effective_image_sha256,effective_resources_json,enable_internet,startup_json,startup_envelope_json)
                VALUES (?1,?2,?3,?4,1,1,?5,'admitting',30,30,?6,?7,?8,?9,?10)",
                params![authority.application_id,authority.namespace_resource_id,
                authority.object_id.to_string(),authority.object_generation,&key,spec.image_sha256.as_slice(),
                serde_json::to_vec(&spec.resources).unwrap(),spec.enable_internet,
                serde_json::to_vec(&startup).unwrap(),spec.startup_envelope]).unwrap();
            Ok(())
        }).unwrap();
        assert!(
            repo.application_leases(authority.instance_id, authority.application_id)
                .is_err()
        );
        assert!(repo.existing_lease(authority).is_err());
    }
}

#[test]
fn default_target_rejects_explicit_image_or_resource_authority_before_reservation() {
    let fixture = fixture(4);
    let repo = ContainerAuthority::new(fixture.storage.db());
    let key = "a".repeat(64);
    let base = execution(&fixture.storage, &fixture.authority, &fixture.spec, &key);
    for change in 0..5 {
        let mut supplied = base.clone();
        match change {
            0 => supplied.startup.image_override = true,
            1 => supplied.startup.resources_override = true,
            2 => {
                supplied.startup.instance_profile =
                    Some(open_compute_core::containers::ContainerInstanceProfile::Lite);
            }
            3 => supplied.resources.cpu_nanos /= 2,
            _ => supplied.image_sha256 = [9; 32],
        }
        assert!(
            repo.reserve(&fixture.authority, &key, &supplied, 4, 30)
                .is_err()
        );
        assert!(repo.existing_lease(&fixture.authority).unwrap().is_none());
    }
    assert_eq!(
        repo.reserve(&fixture.authority, &key, &base, 4, 31)
            .unwrap()
            .effective_image_sha256,
        fixture.spec.image_sha256.unwrap()
    );
}

#[test]
fn durable_object_implicit_resources_require_lite_before_reserving_capacity() {
    let fixture = fixture_with_policy(4, true);
    let repo = ContainerAuthority::new(fixture.storage.db());
    let key = "a".repeat(64);
    let mut supplied = execution(&fixture.storage, &fixture.authority, &fixture.spec, &key);
    supplied.startup.resources_override = false;
    assert!(
        repo.reserve(&fixture.authority, &key, &supplied, 10, 30)
            .is_err()
    );
    assert!(repo.existing_lease(&fixture.authority).unwrap().is_none());
    supplied.resources = open_compute_core::containers::ContainerInstanceProfile::Lite
        .resources(fixture.spec.resources)
        .unwrap();
    let lease = repo
        .reserve(&fixture.authority, &key, &supplied, 10, 31)
        .unwrap();
    assert_eq!(lease.effective_resources, supplied.resources);
    assert!(!lease.startup.resources_override);
    assert_eq!(
        repo.reserve(&fixture.authority, &"b".repeat(64), &supplied, 10, 32)
            .unwrap(),
        lease
    );
}
