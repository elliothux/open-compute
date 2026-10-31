use super::*;
use crate::PlatformStorage;
use crate::durable_objects::{DO_NAMESPACE_SCHEMA_VERSION, DurableObjectRepository};
use crate::resources::{ReserveResourceCreate, ResourceCreateReservation, ResourceRepository};
use crate::worker_repository::{
    EffectiveResourceLimits, NewVersion, NewVersionProducts, WorkerRepository,
};
use open_compute_core::config::DataConfig;
use open_compute_core::{
    BindingKind, RequestId, SystemClock, WorkerId, durable_object_namespace_prefix,
};
use std::collections::BTreeMap;

struct Fixture {
    directory: tempfile::TempDir,
    storage: PlatformStorage,
    worker: WorkerId,
    authority: ContainerObjectAuthority,
    spec: ContainerTargetSpec,
}

fn publish_target(
    repo: ContainerAuthority<'_>,
    instance: InstanceId,
    application: ResourceId,
    spec: &ContainerTargetSpec,
    configuration: &[u8],
    now_ms: i64,
) -> Result<ContainerTarget, PlatformError> {
    let app = repo
        .applications(instance)?
        .into_iter()
        .find(|app| app.id == application)
        .ok_or_else(not_found)?;
    repo.commit_application_configuration(
        instance,
        &app,
        spec,
        configuration,
        ContainerApplicationMetadataPatch {
            max_instances: None,
            observability_logs: Some(false),
        },
        now_ms,
    )
}

fn execution(
    storage: &PlatformStorage,
    authority: &ContainerObjectAuthority,
    target: &ContainerTargetSpec,
    key: &str,
) -> ContainerExecutionSpec {
    let instance = storage.identity().instance_id;
    let owner = ContainerLeaseIdentity {
        namespace_resource_id: authority.namespace_resource_id,
        object_id: authority.object_id,
        object_generation: authority.object_generation,
        lease_generation: 1,
    }
    .owner_key(instance);
    // Invalid-key scenarios still reach the production reservation validator.
    let encryption_key = if valid_opaque(key) {
        key.to_owned()
    } else {
        "0".repeat(64)
    };
    let options = serde_json::json!({"enableInternet":target.enable_internet,
        "entrypoint":null,"environmentVariables":[],"labels":[]});
    let envelope = storage
        .crypto()
        .seal_container_startup(
            &open_compute_core::SecretBytes::new(serde_json::to_vec(&options).unwrap()),
            instance,
            &owner,
            &encryption_key,
        )
        .unwrap();
    ContainerExecutionSpec {
        directory_snapshots: Vec::new(),
        image_sha256: target.image_sha256.unwrap(),
        resources: target.resources,
        enable_internet: target.enable_internet,
        startup: ContainerLeaseStartup {
            image_reference: format!(
                "registry/app@sha256:{}",
                hex::encode(target.image_sha256.unwrap())
            ),
            resources_override: ContainerAuthority::new(storage.db())
                .applications(instance)
                .unwrap()
                .iter()
                .any(|app| {
                    app.id == authority.application_id
                        && app.scheduling_policy == ContainerSchedulingPolicy::DurableObject
                }),
            ..ContainerLeaseStartup::default()
        },
        startup_envelope: serde_json::to_vec(&envelope).unwrap(),
    }
}

fn configuration(root: &std::path::Path) -> DataConfig {
    DataConfig {
        path: root.to_owned(),
        master_key_file: root.join("keys/master.key"),
        master_key_env: None,
        sqlite_busy_timeout_ms: 5000,
        free_space_soft_bytes: 1_073_741_824,
        free_space_hard_bytes: 268_435_456,
    }
}

fn declaration_digest(namespace: ResourceId, name: &str) -> [u8; 32] {
    VersionContainerDeclaration::new(namespace, name.to_owned())
        .unwrap()
        .descriptor_sha256
}

fn image(fill: u8) -> ContainerImage {
    ContainerImage {
        manifest_sha256: [fill; 32],
        config_sha256: [fill + 1; 32],
        diff_ids: vec![[fill + 3; 32], [fill + 4; 32]],
        architecture: "arm64".into(),
        compressed_bytes: 100,
        unpacked_bytes: 200,
        artifact_sha256: [fill + 2; 32],
        artifact_size: 2048,
        created_at_ms: 1,
    }
}

fn version(storage: &PlatformStorage, worker: WorkerId) -> VersionId {
    let id = VersionId::generate();
    WorkerRepository::new(storage.db())
        .insert_staging_version(
            &NewVersion {
                id,
                instance_id: storage.identity().instance_id,
                worker_id: worker,
                content_kind: crate::worker_repository::VersionContentKind::Worker,
                artifact_sha256: Some([1; 32]),
                artifact_size: Some(1),
                artifact_schema_version: Some(1),
                main_module: Some("index.js".into()),
                worker_code_sha256: [9; 32],
                compatibility_date: "2026-09-08".into(),
                compatibility_flags: Vec::new(),
                resource_limits: EffectiveResourceLimits::standard_defaults(),
                vars: BTreeMap::new(),
                secrets: BTreeMap::new(),
                request_id: RequestId::generate(),
                now_ms: 13,
            },
            &NewVersionProducts::default(),
            1_000_000,
        )
        .unwrap();
    id
}

fn activate(storage: &PlatformStorage, worker: WorkerId, id: VersionId) -> u64 {
    let workers = WorkerRepository::new(storage.db());
    workers.begin_validation(id).unwrap();
    workers.mark_ready(id, 14).unwrap();
    workers
        .promote(
            storage.identity().instance_id,
            worker,
            id,
            None,
            RequestId::generate(),
            15,
        )
        .unwrap()
        .route_generation
}

fn insert_object(storage: &PlatformStorage, namespace: ResourceId, fill: u8) -> DurableObjectId {
    let mut bytes = [fill; 32];
    bytes[..8].copy_from_slice(&durable_object_namespace_prefix(namespace));
    let object = DurableObjectId::for_namespace(bytes, namespace).unwrap();
    storage.db().with_immediate(|tx| {
        tx.execute("INSERT INTO do_objects(namespace_resource_id,object_id,generation,state,created_at_ms,updated_at_ms)
                    VALUES (?1,?2,1,'ready',1,1)",params![namespace,object.to_string()]).map_err(|_| invariant())?;
        Ok(())
    }).unwrap();
    object
}

#[path = "containers_fixture.rs"]
mod fixture_support;
use fixture_support::fixture_with_policy;

fn fixture(max_instances: u32) -> Fixture {
    fixture_with_policy(max_instances, false)
}

fn observe<'a>(
    key: &'a str,
    start: &'a str,
    spec: &'a ContainerTargetSpec,
) -> ContainerRunningObservation<'a> {
    ContainerRunningObservation {
        opaque_engine_key: key,
        start_identity: start,
        image_sha256: spec.image_sha256.as_ref().unwrap(),
        provider_contract_sha256: &spec.provider_contract_sha256,
    }
}

fn termination<'a>(key: &'a str, start: Option<&'a str>) -> ContainerTermination<'a> {
    ContainerTermination {
        opaque_engine_key: key,
        start_identity: start,
        terminated: true,
        cleanup_complete: true,
        late_operations_fenced: true,
    }
}

#[test]
fn uncertain_start_retains_capacity_and_identity_until_cleanup_is_proven() {
    let fixture = fixture(1);
    let repo = ContainerAuthority::new(fixture.storage.db());
    let key = "a".repeat(64);
    let start = "b".repeat(64);
    assert!(repo.existing_lease(&fixture.authority).unwrap().is_none());
    let lease = repo
        .reserve(
            &fixture.authority,
            &key,
            &execution(&fixture.storage, &fixture.authority, &fixture.spec, &key),
            10,
            30,
        )
        .unwrap();
    assert_eq!(lease.state, ContainerLeaseState::Admitting);
    assert!(
        repo.observe_running(lease.identity, &observe(&key, &start, &fixture.spec), 31)
            .is_err()
    );
    repo.mark_starting(lease.identity, &key, 31).unwrap();
    repo.mark_starting(lease.identity, &key, 32).unwrap();
    repo.mark_lost(lease.identity, &key, 33).unwrap();
    repo.mark_lost(lease.identity, &key, 34).unwrap();
    assert!(repo.mark_starting(lease.identity, &key, 35).is_err());
    let same = repo
        .reserve(
            &fixture.authority,
            &"c".repeat(64),
            &execution(
                &fixture.storage,
                &fixture.authority,
                &fixture.spec,
                &"c".repeat(64),
            ),
            10,
            36,
        )
        .unwrap();
    assert_eq!(same.identity, lease.identity);
    assert_eq!(same.opaque_engine_key, key);
    assert_eq!(same.state, ContainerLeaseState::Lost);
    assert_eq!(repo.existing_lease(&fixture.authority).unwrap(), Some(same));
    let mut other = fixture.authority;
    other.object_id = insert_object(&fixture.storage, other.namespace_resource_id, 8);
    assert_eq!(
        repo.reserve(
            &other,
            &"d".repeat(64),
            &execution(&fixture.storage, &other, &fixture.spec, &"d".repeat(64)),
            10,
            37
        )
        .unwrap_err()
        .code(),
        ErrorCode::AdmissionBusy
    );
    for missing in 0..3 {
        let mut proof = termination(&key, None);
        match missing {
            0 => proof.terminated = false,
            1 => proof.cleanup_complete = false,
            _ => proof.late_operations_fenced = false,
        }
        assert!(repo.finish_stopped(lease.identity, &proof, 38).is_err());
    }
    assert!(
        repo.finish_stopped(lease.identity, &termination(&"f".repeat(64), None), 38)
            .is_err()
    );
    repo.observe_running(lease.identity, &observe(&key, &start, &fixture.spec), 39)
        .unwrap();
    assert_eq!(
        repo.lease(fixture.authority.instance_id, lease.identity)
            .unwrap()
            .state,
        ContainerLeaseState::Running
    );
    let different_start = "c".repeat(64);
    let bad = observe(&key, &different_start, &fixture.spec);
    assert!(repo.observe_running(lease.identity, &bad, 40).is_err());
    assert!(
        repo.finish_stopped(lease.identity, &termination(&key, None), 41)
            .is_err()
    );
    repo.begin_stop(lease.identity, &key, 42).unwrap();
    repo.begin_stop(lease.identity, &key, 43).unwrap();
    repo.mark_lost(lease.identity, &key, 43).unwrap();
    assert_eq!(
        repo.lease(fixture.authority.instance_id, lease.identity)
            .unwrap()
            .stop_requested_at_ms,
        Some(42)
    );
    repo.lose_all(fixture.authority.instance_id, 43).unwrap();
    assert!(fixture.storage.db().with_immediate(|tx| {
        tx.execute("UPDATE container_instance_leases SET stop_requested_at_ms=NULL WHERE opaque_engine_key=?1",[&key])
          .map(|_| ()).map_err(|_| PlatformError::new(ErrorCode::MigrationFailed,"immutable stop intent"))
    }).is_err());
    assert!(
        repo.observe_running(lease.identity, &observe(&key, &start, &fixture.spec), 44)
            .is_err()
    );
    repo.finish_stopped(lease.identity, &termination(&key, Some(&start)), 45)
        .unwrap();
    repo.finish_stopped(lease.identity, &termination(&key, Some(&start)), 46)
        .unwrap();
    assert!(repo.existing_lease(&fixture.authority).unwrap().is_none());
    repo.set_admission_paused(
        fixture.authority.instance_id,
        fixture.authority.application_id,
        false,
    )
    .unwrap();
    let next = repo
        .reserve(
            &fixture.authority,
            &"e".repeat(64),
            &execution(
                &fixture.storage,
                &fixture.authority,
                &fixture.spec,
                &"e".repeat(64),
            ),
            10,
            47,
        )
        .unwrap();
    assert_eq!(
        next.identity.lease_generation,
        lease.identity.lease_generation + 1
    );
    assert!(repo.begin_stop(lease.identity, &key, 48).is_err());
    assert!(repo.mark_lost(lease.identity, &key, 49).is_err());
    assert_eq!(
        repo.lease(fixture.authority.instance_id, lease.identity)
            .unwrap()
            .stop_requested_at_ms,
        Some(42)
    );
    assert_eq!(
        repo.lease(fixture.authority.instance_id, next.identity)
            .unwrap()
            .state,
        ContainerLeaseState::Admitting
    );
}

#[test]
fn code_and_target_updates_preserve_old_effective_lease_and_revoke_old_facet() {
    let fixture = fixture(2);
    let repo = ContainerAuthority::new(fixture.storage.db());
    let key = "a".repeat(64);
    let start = "b".repeat(64);
    let lease = repo
        .reserve(
            &fixture.authority,
            &key,
            &execution(&fixture.storage, &fixture.authority, &fixture.spec, &key),
            10,
            30,
        )
        .unwrap();
    repo.mark_starting(lease.identity, &key, 31).unwrap();
    repo.observe_running(lease.identity, &observe(&key, &start, &fixture.spec), 32)
        .unwrap();
    let new_version = version(&fixture.storage, fixture.worker);
    repo.declare_version(
        fixture.authority.instance_id,
        new_version,
        fixture.authority.namespace_resource_id,
        "container-app",
        &declaration_digest(fixture.authority.namespace_resource_id, "container-app"),
    )
    .unwrap();
    let route = activate(&fixture.storage, fixture.worker, new_version);
    let mut spec = fixture.spec.clone();
    spec.worker_version_id = new_version;
    let target = publish_target(
        repo,
        fixture.authority.instance_id,
        fixture.authority.application_id,
        &spec,
        b"{}",
        33,
    )
    .unwrap();
    assert_eq!(target.generation, 1);
    assert_eq!(
        target.spec.worker_version_id,
        fixture.spec.worker_version_id
    );
    fixture.storage.db().with_read(|conn| {
        let references: Vec<String> = conn.prepare(
            "SELECT version_id FROM version_referrers WHERE kind='container_target' ORDER BY version_id"
        ).unwrap().query_map([], |row| row.get(0)).unwrap().collect::<Result<_,_>>().unwrap();
        assert_eq!(references, vec![fixture.spec.worker_version_id.to_string()]);
        Ok(())
    }).unwrap();
    repo.admit_image(fixture.authority.instance_id, &image(4))
        .unwrap();
    spec.image_sha256 = Some([4; 32]);
    assert_eq!(
        publish_target(
            repo,
            fixture.authority.instance_id,
            fixture.authority.application_id,
            &spec,
            b"{}",
            34
        )
        .unwrap()
        .generation,
        2
    );
    assert_eq!(
        repo.reserve(
            &fixture.authority,
            &"c".repeat(64),
            &execution(
                &fixture.storage,
                &fixture.authority,
                &fixture.spec,
                &"c".repeat(64)
            ),
            10,
            35
        )
        .unwrap_err()
        .code(),
        ErrorCode::DoVersionStale
    );
    let mut current = fixture.authority;
    current.route_generation = route;
    assert_eq!(
        repo.reserve(
            &current,
            &"c".repeat(64),
            &execution(&fixture.storage, &current, &fixture.spec, &"c".repeat(64)),
            10,
            36
        )
        .unwrap(),
        repo.lease(current.instance_id, lease.identity).unwrap()
    );
    assert_eq!(
        repo.lease(current.instance_id, lease.identity)
            .unwrap()
            .target_generation,
        1
    );
    repo.mark_lost(lease.identity, &key, 37).unwrap();
    assert!(
        repo.observe_running(lease.identity, &observe(&key, &start, &spec), 38)
            .is_err()
    );
    repo.observe_running(lease.identity, &observe(&key, &start, &fixture.spec), 39)
        .unwrap();
    let mut other = current;
    other.object_id = insert_object(&fixture.storage, other.namespace_resource_id, 8);
    assert_eq!(
        repo.reserve(
            &other,
            &"d".repeat(64),
            &execution(&fixture.storage, &other, &spec, &"d".repeat(64)),
            10,
            40
        )
        .unwrap()
        .target_generation,
        2
    );
    let rollback = publish_target(
        repo,
        current.instance_id,
        current.application_id,
        &fixture.spec,
        b"{}",
        41,
    )
    .unwrap();
    assert_eq!(rollback.generation, 3);
    assert_eq!(rollback.spec.image_sha256, Some([1; 32]));
    assert_eq!(
        repo.lease(current.instance_id, lease.identity)
            .unwrap()
            .target_generation,
        1
    );
}

#[test]
fn database_restart_preserves_lost_reservations_and_pauses_new_admission() {
    let fixture = fixture(3);
    let key = "a".repeat(64);
    let start = "b".repeat(64);
    let authority = fixture.authority;
    let lease = ContainerAuthority::new(fixture.storage.db())
        .reserve(
            &authority,
            &key,
            &execution(&fixture.storage, &authority, &fixture.spec, &key),
            10,
            30,
        )
        .unwrap();
    let repo = ContainerAuthority::new(fixture.storage.db());
    repo.mark_starting(lease.identity, &key, 31).unwrap();
    repo.observe_running(lease.identity, &observe(&key, &start, &fixture.spec), 32)
        .unwrap();
    let mut other = authority;
    other.object_id = insert_object(&fixture.storage, other.namespace_resource_id, 8);
    assert!(
        repo.reserve(
            &other,
            &"c".repeat(64),
            &execution(&fixture.storage, &other, &fixture.spec, &"c".repeat(64)),
            1,
            33
        )
        .is_err()
    );
    let path = fixture.directory.path().join("data");
    drop(fixture.storage);
    let storage = PlatformStorage::bootstrap(&configuration(&path), &SystemClock).unwrap();
    let repo = ContainerAuthority::new(storage.db());
    assert_eq!(
        repo.image(authority.instance_id, &image(1).manifest_sha256)
            .unwrap(),
        image(1)
    );
    assert_eq!(repo.lose_all(authority.instance_id, 34).unwrap(), 1);
    assert_eq!(repo.lose_all(authority.instance_id, 35).unwrap(), 0);
    assert_eq!(
        repo.reserve(
            &authority,
            &"c".repeat(64),
            &execution(&storage, &authority, &fixture.spec, &"c".repeat(64)),
            10,
            36
        )
        .unwrap()
        .state,
        ContainerLeaseState::Lost
    );
    assert!(
        repo.reserve(
            &other,
            &"d".repeat(64),
            &execution(&storage, &other, &fixture.spec, &"d".repeat(64)),
            10,
            37
        )
        .is_err()
    );
    let mut wrong_contract = fixture.spec.clone();
    wrong_contract.provider_contract_sha256 = [0; 32];
    assert!(
        repo.observe_running(lease.identity, &observe(&key, &start, &wrong_contract), 38)
            .is_err()
    );
    repo.observe_running(lease.identity, &observe(&key, &start, &fixture.spec), 39)
        .unwrap();
    assert!(
        repo.reserve(
            &other,
            &"d".repeat(64),
            &execution(&storage, &other, &fixture.spec, &"d".repeat(64)),
            10,
            40
        )
        .is_err()
    );
    repo.set_admission_paused(authority.instance_id, authority.application_id, false)
        .unwrap();
    assert!(
        repo.reserve(
            &other,
            &"d".repeat(64),
            &execution(&storage, &other, &fixture.spec, &"d".repeat(64)),
            10,
            41
        )
        .is_ok()
    );
    assert!(repo.lose_all(InstanceId::generate(), 42).is_err());
}

#[test]
fn concurrent_admission_cannot_exceed_application_capacity() {
    let fixture = fixture(1);
    let mut other = fixture.authority;
    other.object_id = insert_object(&fixture.storage, other.namespace_resource_id, 8);
    let repo = ContainerAuthority::new(fixture.storage.db());
    std::thread::scope(|scope| {
        let first = scope.spawn(|| {
            repo.reserve(
                &fixture.authority,
                &"a".repeat(64),
                &execution(
                    &fixture.storage,
                    &fixture.authority,
                    &fixture.spec,
                    &"a".repeat(64),
                ),
                10,
                30,
            )
        });
        let second = scope.spawn(|| {
            repo.reserve(
                &other,
                &"b".repeat(64),
                &execution(&fixture.storage, &other, &fixture.spec, &"b".repeat(64)),
                10,
                30,
            )
        });
        let results = [first.join().unwrap(), second.join().unwrap()];
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert_eq!(
            results
                .iter()
                .filter(|result| result
                    .as_ref()
                    .is_err_and(|error| error.code() == ErrorCode::AdmissionBusy))
                .count(),
            1
        );
    });
}

#[test]
fn authority_rejects_undeclared_classes_wrong_instances_and_provider_identities() {
    let fixture = fixture(2);
    let repo = ContainerAuthority::new(fixture.storage.db());
    for field in 0..6 {
        let mut authority = fixture.authority;
        match field {
            0 => authority.instance_id = InstanceId::generate(),
            1 => authority.application_id = ResourceId::generate(),
            2 => authority.namespace_resource_id = ResourceId::generate(),
            3 => authority.object_generation += 1,
            4 => authority.route_generation += 1,
            _ => authority.descriptor_sha256 = [0; 32],
        }
        assert!(
            repo.reserve(
                &authority,
                &"a".repeat(64),
                &execution(&fixture.storage, &authority, &fixture.spec, &"a".repeat(64)),
                10,
                30
            )
            .is_err()
        );
    }
    for key in ["short", "A".repeat(64).as_str()] {
        assert!(
            repo.reserve(
                &fixture.authority,
                key,
                &execution(&fixture.storage, &fixture.authority, &fixture.spec, key),
                10,
                30
            )
            .is_err()
        );
    }
    assert!(
        repo.reserve(
            &fixture.authority,
            &"a".repeat(64),
            &execution(
                &fixture.storage,
                &fixture.authority,
                &fixture.spec,
                &"a".repeat(64)
            ),
            0,
            30
        )
        .is_err()
    );
    let lease = repo
        .reserve(
            &fixture.authority,
            &"a".repeat(64),
            &execution(
                &fixture.storage,
                &fixture.authority,
                &fixture.spec,
                &"a".repeat(64),
            ),
            10,
            30,
        )
        .unwrap();
    assert!(
        repo.mark_starting(lease.identity, &"b".repeat(64), 31)
            .is_err()
    );
    assert!(repo.lease(InstanceId::generate(), lease.identity).is_err());
    let mut nonexistent = lease.identity;
    nonexistent.lease_generation += 1;
    assert!(
        repo.lease(fixture.authority.instance_id, nonexistent)
            .is_err()
    );
    assert!(
        repo.observe_running(
            lease.identity,
            &observe(&"a".repeat(64), "bad", &fixture.spec),
            32
        )
        .is_err()
    );
    assert!(
        repo.finish_stopped(
            lease.identity,
            &termination(&"a".repeat(64), Some("bad")),
            33
        )
        .is_err()
    );
    repo.finish_stopped(lease.identity, &termination(&"a".repeat(64), None), 34)
        .unwrap();
}

#[test]
fn immutable_admission_metadata_and_sql_guards_fail_closed() {
    let fixture = fixture(2);
    let repo = ContainerAuthority::new(fixture.storage.db());
    let instance = fixture.authority.instance_id;
    repo.admit_image(instance, &image(1)).unwrap();
    let mut mismatch = image(1);
    mismatch.config_sha256 = [99; 32];
    assert!(repo.admit_image(instance, &mismatch).is_err());
    let mut mismatch = image(1);
    mismatch.artifact_size += 1;
    assert!(repo.admit_image(instance, &mismatch).is_err());
    assert!(
        WorkerRepository::new(fixture.storage.db())
            .referenced_artifacts()
            .unwrap()
            .contains(&(image(1).artifact_sha256, image(1).artifact_size))
    );
    assert!(repo.admit_image(InstanceId::generate(), &image(4)).is_err());
    for (fill, count) in [(5, 0), (6, 128)] {
        let mut valid = image(fill);
        valid.diff_ids = vec![[7; 32]; count];
        repo.admit_image(instance, &valid).unwrap();
        assert_eq!(repo.image(instance, &valid.manifest_sha256).unwrap(), valid);
    }
    let mut reordered = image(1);
    reordered.diff_ids.reverse();
    assert!(repo.admit_image(instance, &reordered).is_err());
    assert_eq!(
        repo.image(instance, &image(1).manifest_sha256).unwrap(),
        image(1)
    );
    for field in 0..8 {
        let mut invalid = image(4);
        match field {
            0 => invalid.architecture = "unknown".into(),
            1 => invalid.compressed_bytes = 0,
            2 => invalid.unpacked_bytes = 0,
            3 => invalid.compressed_bytes = u64::MAX,
            4 => invalid.unpacked_bytes = u64::MAX,
            5 => invalid.artifact_size = 0,
            6 => invalid.artifact_size = u64::MAX,
            _ => invalid.diff_ids = vec![[1; 32]; 129],
        }
        assert!(repo.admit_image(instance, &invalid).is_err());
    }
    fixture.storage.db().with_read(|conn| {
        for size in [1, 31, 33, 4097, 4128] {
            assert!(conn.execute("INSERT INTO container_images(manifest_sha256,config_sha256,architecture,compressed_bytes,unpacked_bytes,artifact_sha256,artifact_size,created_at_ms,diff_ids) SELECT ?1,config_sha256,architecture,compressed_bytes,unpacked_bytes,artifact_sha256,artifact_size,created_at_ms,zeroblob(?2) FROM container_images LIMIT 1", params![[98_u8;32],size]).is_err());
        }
        assert!(conn.execute("INSERT INTO container_images(manifest_sha256,config_sha256,architecture,compressed_bytes,unpacked_bytes,artifact_sha256,artifact_size,created_at_ms,diff_ids) SELECT ?1,config_sha256,architecture,compressed_bytes,unpacked_bytes,artifact_sha256,artifact_size,created_at_ms,NULL FROM container_images LIMIT 1", params![[98_u8;32]]).is_err());
        conn.pragma_update(None, "ignore_check_constraints", true).unwrap();
        for (fill, size) in [(99_u8, 33), (100, 4128)] {
            conn.execute("INSERT INTO container_images(manifest_sha256,config_sha256,architecture,compressed_bytes,unpacked_bytes,artifact_sha256,artifact_size,created_at_ms,diff_ids) SELECT ?1,config_sha256,architecture,compressed_bytes,unpacked_bytes,artifact_sha256,artifact_size,created_at_ms,zeroblob(?2) FROM container_images LIMIT 1", params![[fill;32],size]).unwrap();
        }
        conn.pragma_update(None, "ignore_check_constraints", false).unwrap();
        Ok(())
    }).unwrap();
    for fill in [99, 100] {
        assert_eq!(
            repo.image(instance, &[fill; 32]).unwrap_err().code(),
            ErrorCode::ResourceInvariantViolation
        );
    }
    for name in ["", "bad/name", "bad name", &"x".repeat(64)] {
        assert!(
            repo.create_application(
                instance,
                ResourceId::generate(),
                fixture.authority.namespace_resource_id,
                name,
                1,
                30
            )
            .is_err()
        );
    }
    assert!(
        repo.create_application(
            instance,
            ResourceId::generate(),
            fixture.authority.namespace_resource_id,
            "new-app",
            0,
            30
        )
        .is_err()
    );
    assert!(
        repo.create_application(
            instance,
            ResourceId::generate(),
            ResourceId::generate(),
            "new-app",
            1,
            30
        )
        .is_err()
    );
    repo.create_application(
        instance,
        fixture.authority.application_id,
        fixture.authority.namespace_resource_id,
        "container-app",
        2,
        30,
    )
    .unwrap();
    assert!(
        repo.create_application(
            instance,
            fixture.authority.application_id,
            fixture.authority.namespace_resource_id,
            "changed-name",
            2,
            30
        )
        .is_err()
    );
    assert!(
        repo.create_application(
            instance,
            ResourceId::generate(),
            fixture.authority.namespace_resource_id,
            "duplicate",
            2,
            30
        )
        .is_err()
    );
    let undeclared = version(&fixture.storage, fixture.worker);
    let mut spec = fixture.spec.clone();
    spec.worker_version_id = undeclared;
    assert!(
        publish_target(
            repo,
            instance,
            fixture.authority.application_id,
            &spec,
            b"{}",
            30
        )
        .is_err()
    );
    assert!(
        repo.declare_version(
            instance,
            fixture.spec.worker_version_id,
            fixture.authority.namespace_resource_id,
            "container-app",
            &declaration_digest(fixture.authority.namespace_resource_id, "container-app")
        )
        .is_err()
    );
    assert!(
        repo.declare_version(
            instance,
            undeclared,
            fixture.authority.namespace_resource_id,
            "container-app",
            &declaration_digest(fixture.authority.namespace_resource_id, "container-app")
        )
        .is_ok()
    );
    assert!(
        repo.declare_version(
            instance,
            undeclared,
            fixture.authority.namespace_resource_id,
            "container-app",
            &declaration_digest(fixture.authority.namespace_resource_id, "container-app")
        )
        .is_ok()
    );
    assert!(
        repo.declare_version(
            instance,
            undeclared,
            fixture.authority.namespace_resource_id,
            "container-app",
            &[0; 32]
        )
        .is_err()
    );
    assert!(
        repo.set_admission_paused(instance, ResourceId::generate(), false)
            .is_err()
    );
    assert!(
        repo.target(instance, fixture.authority.application_id, 99)
            .is_err()
    );
    assert!(
        repo.target(InstanceId::generate(), fixture.authority.application_id, 1)
            .is_err()
    );
    spec.resources.pids = 0;
    assert!(
        publish_target(
            repo,
            instance,
            fixture.authority.application_id,
            &spec,
            b"{}",
            30
        )
        .is_err()
    );
    for statement in [
        "UPDATE container_images SET architecture='amd64'",
        "UPDATE container_application_targets SET generation=99",
        "UPDATE container_applications SET target_generation=NULL",
        "UPDATE version_containers SET descriptor_sha256=zeroblob(32)",
        "DELETE FROM version_containers WHERE version_id IN (SELECT id FROM worker_versions WHERE state='ready')",
    ] {
        assert!(
            fixture
                .storage
                .db()
                .with_immediate(|tx| {
                    tx.execute(statement, []).map_err(|_| invariant())?;
                    Ok(())
                })
                .is_err()
        );
    }
    let lease = repo
        .reserve(
            &fixture.authority,
            &"a".repeat(64),
            &execution(
                &fixture.storage,
                &fixture.authority,
                &fixture.spec,
                &"a".repeat(64),
            ),
            10,
            30,
        )
        .unwrap();
    for statement in [
        "UPDATE container_instance_leases SET target_generation=99",
        "UPDATE container_instance_leases SET opaque_engine_key=printf('%064d',1)",
        "UPDATE container_instance_leases SET state='stopped'",
    ] {
        assert!(
            fixture
                .storage
                .db()
                .with_immediate(|tx| {
                    tx.execute(statement, []).map_err(|_| invariant())?;
                    Ok(())
                })
                .is_err()
        );
    }
    repo.finish_stopped(lease.identity, &termination(&"a".repeat(64), None), 40)
        .unwrap();
    assert!(
        fixture
            .storage
            .db()
            .with_immediate(|tx| {
                tx.execute(
                    "UPDATE container_instance_leases SET state='lost',stopped_at_ms=NULL",
                    [],
                )
                .map_err(|_| invariant())?;
                Ok(())
            })
            .is_err()
    );
}

#[test]
fn container_declarations_are_atomic_and_canonical_before_application_creation() {
    let fixture = fixture(2);
    let instance = fixture.storage.identity().instance_id;
    let namespace = fixture.authority.namespace_resource_id;
    let declaration = VersionContainerDeclaration::new(namespace, "next-app".into()).unwrap();
    declaration.validate().unwrap();
    for name in ["", "has/slash", "with space"] {
        assert!(VersionContainerDeclaration::new(namespace, name.into()).is_err());
    }
    let mut corrupt = declaration.clone();
    corrupt.descriptor_sha256[0] ^= 1;
    assert!(corrupt.validate().is_err());
    let id = version(&fixture.storage, fixture.worker);
    fixture
        .storage
        .db()
        .with_immediate(|tx| {
            insert_version_declarations(tx, id, std::slice::from_ref(&declaration), None)
        })
        .unwrap();
    let repo = ContainerAuthority::new(fixture.storage.db());
    assert_eq!(
        repo.version_declarations(instance, id).unwrap(),
        vec![declaration.clone()]
    );
    assert!(
        repo.version_declarations(InstanceId::generate(), id)
            .is_err()
    );
    assert!(
        repo.application_for_namespace(instance, namespace, "next-app")
            .is_err()
    );
    let applications = repo.applications(instance).unwrap();
    assert_eq!(applications.len(), 1);
    assert_eq!(applications[0].id, fixture.authority.application_id);
    assert_eq!(
        repo.application_for_namespace(instance, namespace, "container-app")
            .unwrap(),
        applications[0]
    );
    assert!(repo.applications(InstanceId::generate()).is_err());
    assert_eq!(repo.image(instance, &[1; 32]).unwrap(), image(1));
    assert!(repo.image(instance, &[99; 32]).is_err());
    assert!(repo.image(InstanceId::generate(), &[1; 32]).is_err());
    let second = version(&fixture.storage, fixture.worker);
    assert!(
        fixture
            .storage
            .db()
            .with_immediate(|tx| {
                insert_version_declarations(
                    tx,
                    second,
                    &[declaration.clone(), corrupt.clone()],
                    None,
                )
            })
            .is_err()
    );
    assert!(
        repo.version_declarations(instance, second)
            .unwrap()
            .is_empty()
    );
    activate(&fixture.storage, fixture.worker, second);
    assert!(
        fixture
            .storage
            .db()
            .with_immediate(|tx| {
                insert_version_declarations(tx, second, std::slice::from_ref(&declaration), None)
            })
            .is_err()
    );
}

#[test]
fn container_rollouts_freeze_execution_cohorts_and_wait_for_positive_cleanup() {
    let fixture = fixture(3);
    let instance = fixture.storage.identity().instance_id;
    let repo = ContainerAuthority::new(fixture.storage.db());
    let first = repo
        .reserve(
            &fixture.authority,
            &"a".repeat(64),
            &execution(
                &fixture.storage,
                &fixture.authority,
                &fixture.spec,
                &"a".repeat(64),
            ),
            10,
            30,
        )
        .unwrap();
    let mut second_authority = fixture.authority;
    second_authority.object_id =
        insert_object(&fixture.storage, fixture.authority.namespace_resource_id, 8);
    let second = repo
        .reserve(
            &second_authority,
            &"b".repeat(64),
            &execution(
                &fixture.storage,
                &second_authority,
                &fixture.spec,
                &"b".repeat(64),
            ),
            10,
            31,
        )
        .unwrap();
    repo.admit_image(instance, &image(2)).unwrap();
    let mut spec = fixture.spec.clone();
    spec.image_sha256 = Some([2; 32]);
    let app = repo
        .application_for_namespace(
            instance,
            fixture.authority.namespace_resource_id,
            "container-app",
        )
        .unwrap();
    repo.commit_application_configuration(
        instance,
        &app,
        &spec,
        b"{}",
        ContainerApplicationMetadataPatch::default(),
        40,
    )
    .unwrap();
    let id = ResourceId::generate();
    let rollout = repo
        .create_rollout(
            instance,
            fixture.authority.application_id,
            &ContainerRolloutCreate {
                id,
                target_configuration: b"{}",
                steps: &[50, 100],
                details: None,
            },
            41,
        )
        .unwrap();
    assert_eq!(rollout.current_step, 0);
    let selected = repo
        .rollout_step_leases(instance, fixture.authority.application_id, id, 42)
        .unwrap();
    assert_eq!(selected.len(), 1);
    assert_eq!(selected[0].identity, first.identity);
    assert_eq!(selected[0].state, ContainerLeaseState::Stopping);
    assert!(selected[0].stop_requested_at_ms.is_some());
    repo.mark_lost(first.identity, &first.opaque_engine_key, 43)
        .unwrap();
    assert_eq!(
        repo.rollout_step_leases(instance, fixture.authority.application_id, id, 44)
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        repo.rollout(instance, fixture.authority.application_id, id)
            .unwrap()
            .current_step,
        0
    );
    repo.finish_stopped(
        first.identity,
        &termination(&first.opaque_engine_key, None),
        45,
    )
    .unwrap();
    let pending = repo
        .rollout_replacements(instance, fixture.authority.application_id, id)
        .unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].identity, first.identity);
    assert_eq!(
        repo.replacement_startup(&fixture.authority)
            .unwrap()
            .unwrap()
            .identity,
        first.identity
    );
    let replacement = repo
        .reserve(
            &fixture.authority,
            &"c".repeat(64),
            &execution(&fixture.storage, &fixture.authority, &spec, &"c".repeat(64)),
            10,
            46,
        )
        .unwrap();
    assert_eq!(replacement.target_generation, 2);
    repo.rollout_step_leases(instance, fixture.authority.application_id, id, 46)
        .unwrap();
    assert_eq!(
        repo.rollout(instance, fixture.authority.application_id, id)
            .unwrap()
            .current_step,
        0
    );
    repo.mark_starting(replacement.identity, &replacement.opaque_engine_key, 46)
        .unwrap();
    repo.observe_running(
        replacement.identity,
        &observe(&replacement.opaque_engine_key, &"d".repeat(64), &spec),
        46,
    )
    .unwrap();
    assert!(
        repo.rollout_step_leases(instance, fixture.authority.application_id, id, 47)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        repo.rollout(instance, fixture.authority.application_id, id)
            .unwrap()
            .current_step,
        1
    );
    let selected = repo
        .rollout_step_leases(instance, fixture.authority.application_id, id, 48)
        .unwrap();
    assert_eq!(selected.len(), 1);
    assert_eq!(selected[0].identity, second.identity);
    repo.finish_stopped(
        second.identity,
        &termination(&second.opaque_engine_key, None),
        49,
    )
    .unwrap();
    repo.rollout_step_leases(instance, fixture.authority.application_id, id, 49)
        .unwrap();
    assert_eq!(
        repo.rollout(instance, fixture.authority.application_id, id)
            .unwrap()
            .state,
        "running"
    );
    let second_replacement = repo
        .reserve(
            &second_authority,
            &"e".repeat(64),
            &execution(&fixture.storage, &second_authority, &spec, &"e".repeat(64)),
            10,
            49,
        )
        .unwrap();
    repo.mark_starting(
        second_replacement.identity,
        &second_replacement.opaque_engine_key,
        49,
    )
    .unwrap();
    repo.observe_running(
        second_replacement.identity,
        &observe(
            &second_replacement.opaque_engine_key,
            &"f".repeat(64),
            &spec,
        ),
        49,
    )
    .unwrap();
    assert!(
        repo.rollout_step_leases(instance, fixture.authority.application_id, id, 50)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        repo.rollout(instance, fixture.authority.application_id, id)
            .unwrap()
            .state,
        "completed"
    );
    assert_eq!(
        repo.lease(instance, replacement.identity).unwrap().state,
        ContainerLeaseState::Running
    );
    assert!(
        repo.rollout_step_leases(instance, fixture.authority.application_id, id, 51)
            .unwrap()
            .is_empty()
    );
    assert!(
        repo.fail_rollout(instance, fixture.authority.application_id, id, 52)
            .is_err()
    );
    for steps in [vec![], vec![0, 100], vec![100, 100], vec![50]] {
        assert!(
            repo.create_rollout(
                instance,
                fixture.authority.application_id,
                &ContainerRolloutCreate {
                    id: ResourceId::generate(),
                    target_configuration: b"{}",
                    steps: &steps,
                    details: None,
                },
                53,
            )
            .is_err()
        );
    }
    let failed = ResourceId::generate();
    repo.create_rollout(
        instance,
        fixture.authority.application_id,
        &ContainerRolloutCreate {
            id: failed,
            target_configuration: b"{}",
            steps: &[100],
            details: None,
        },
        54,
    )
    .unwrap();
    assert!(
        repo.create_rollout(
            instance,
            fixture.authority.application_id,
            &ContainerRolloutCreate {
                id: ResourceId::generate(),
                target_configuration: b"{}",
                steps: &[100],
                details: None,
            },
            55,
        )
        .is_err()
    );
    repo.fail_rollout(instance, fixture.authority.application_id, failed, 56)
        .unwrap();
    assert_eq!(
        repo.rollout(instance, fixture.authority.application_id, failed)
            .unwrap()
            .state,
        "failed"
    );
    assert!(
        repo.rollout(
            InstanceId::generate(),
            fixture.authority.application_id,
            failed
        )
        .is_err()
    );
    assert!(
        repo.rollout(instance, ResourceId::generate(), failed)
            .is_err()
    );
    let app = repo
        .application_for_namespace(
            instance,
            fixture.authority.namespace_resource_id,
            "container-app",
        )
        .unwrap();
    repo.commit_application_configuration(
        instance,
        &app,
        &fixture.spec,
        b"{}",
        ContainerApplicationMetadataPatch {
            max_instances: Some(3),
            observability_logs: Some(false),
        },
        56,
    )
    .unwrap();
    repo.patch_application_metadata(
        instance,
        fixture.authority.application_id,
        ContainerApplicationMetadataPatch {
            max_instances: Some(2),
            observability_logs: Some(true),
        },
        57,
    )
    .unwrap();
    assert_eq!(
        repo.application_settings(instance, fixture.authority.application_id)
            .unwrap(),
        (Some(b"{}".to_vec()), true, 57)
    );
    assert!(
        repo.patch_application_metadata(
            instance,
            fixture.authority.application_id,
            ContainerApplicationMetadataPatch {
                max_instances: Some(0),
                observability_logs: None
            },
            58
        )
        .is_err()
    );
    assert!(
        repo.patch_application_metadata(
            instance,
            ResourceId::generate(),
            ContainerApplicationMetadataPatch::default(),
            58
        )
        .is_err()
    );
    assert!(
        repo.application_settings(instance, ResourceId::generate())
            .is_err()
    );
}

#[test]
fn container_configuration_commit_is_atomic_and_private_routes_validate_declarations() {
    let fixture = fixture(3);
    let repo = ContainerAuthority::new(fixture.storage.db());
    let instance = fixture.storage.identity().instance_id;
    let namespace = fixture.authority.namespace_resource_id;
    let app = repo.applications(instance).unwrap().remove(0);
    assert_eq!(
        repo.resolve_declared_application(
            instance,
            &VersionContainerIdentity {
                namespace_resource_id: namespace,
                application_name: "container-app".to_owned(),
                descriptor_sha256: declaration_digest(
                    fixture.authority.namespace_resource_id,
                    "container-app"
                )
            },
            fixture.authority.object_id,
            1,
            fixture.authority.route_generation,
        )
        .unwrap(),
        Some(app.clone())
    );
    assert!(
        repo.resolve_declared_application(
            instance,
            &VersionContainerIdentity {
                namespace_resource_id: namespace,
                application_name: "container-app".to_owned(),
                descriptor_sha256: declaration_digest(
                    fixture.authority.namespace_resource_id,
                    "container-app"
                )
            },
            fixture.authority.object_id,
            1,
            fixture.authority.route_generation + 1,
        )
        .is_err()
    );
    assert!(
        repo.resolve_declared_application(
            instance,
            &VersionContainerIdentity {
                namespace_resource_id: namespace,
                application_name: "other-app".to_owned(),
                descriptor_sha256: declaration_digest(
                    fixture.authority.namespace_resource_id,
                    "container-app"
                )
            },
            fixture.authority.object_id,
            1,
            fixture.authority.route_generation,
        )
        .is_err()
    );
    assert_eq!(
        repo.active_declaration_version(instance, namespace, "container-app")
            .unwrap(),
        fixture.spec.worker_version_id
    );
    assert!(
        repo.active_declaration_version(instance, namespace, "missing")
            .is_err()
    );
    let mut updated = app.clone();
    updated.max_instances = 2;
    repo.commit_application_configuration(
        instance,
        &updated,
        &fixture.spec,
        b"{\"image\":\"first\"}",
        ContainerApplicationMetadataPatch {
            max_instances: Some(2),
            observability_logs: Some(true),
        },
        30,
    )
    .unwrap();
    let before = repo.application_settings(instance, app.id).unwrap();
    let mut spec = fixture.spec.clone();
    spec.image_sha256 = Some([99; 32]);
    assert!(
        repo.commit_application_configuration(
            instance,
            &updated,
            &spec,
            b"{\"image\":\"bad\"}",
            ContainerApplicationMetadataPatch {
                max_instances: Some(2),
                observability_logs: Some(false)
            },
            31
        )
        .is_err()
    );
    assert_eq!(repo.application_settings(instance, app.id).unwrap(), before);
    assert_eq!(
        repo.applications(instance).unwrap()[0].target_generation,
        Some(2)
    );
    updated.name = "wrong-name".into();
    assert!(
        repo.commit_application_configuration(
            instance,
            &updated,
            &fixture.spec,
            b"{}",
            ContainerApplicationMetadataPatch {
                max_instances: Some(2),
                observability_logs: Some(true)
            },
            32
        )
        .is_err()
    );
    updated = app.clone();
    updated.max_instances = 0;
    assert!(
        repo.commit_application_configuration(
            instance,
            &updated,
            &fixture.spec,
            b"{}",
            ContainerApplicationMetadataPatch {
                max_instances: Some(2),
                observability_logs: Some(true)
            },
            32
        )
        .is_err()
    );
    assert!(
        repo.commit_application_configuration(
            instance,
            &app,
            &fixture.spec,
            b"",
            ContainerApplicationMetadataPatch {
                max_instances: Some(2),
                observability_logs: Some(true)
            },
            32
        )
        .is_err()
    );
    assert!(
        repo.commit_application_configuration(
            instance,
            &app,
            &fixture.spec,
            &vec![b' '; 16385],
            ContainerApplicationMetadataPatch {
                max_instances: Some(2),
                observability_logs: Some(true)
            },
            32
        )
        .is_err()
    );
    assert!(
        repo.commit_application_configuration(
            InstanceId::generate(),
            &app,
            &fixture.spec,
            b"{}",
            ContainerApplicationMetadataPatch {
                max_instances: Some(2),
                observability_logs: Some(true)
            },
            32
        )
        .is_err()
    );
    let next = version(&fixture.storage, fixture.worker);
    repo.declare_version(
        instance,
        next,
        namespace,
        "next-app",
        &declaration_digest(namespace, "next-app"),
    )
    .unwrap();
    let route = activate(&fixture.storage, fixture.worker, next);
    assert!(
        repo.resolve_declared_application(
            instance,
            &VersionContainerIdentity {
                namespace_resource_id: namespace,
                application_name: "next-app".to_owned(),
                descriptor_sha256: declaration_digest(namespace, "next-app")
            },
            fixture.authority.object_id,
            1,
            route,
        )
        .unwrap()
        .is_none()
    );
}

#[path = "containers_registry_tests.rs"]
mod registry;
#[path = "containers_stop_tests.rs"]
mod stop;

#[test]
fn container_owner_fences_instance_namespace_and_object_recreation() {
    let fixture = fixture(2);
    let authority = fixture.authority;
    let identity = ContainerLeaseIdentity {
        namespace_resource_id: authority.namespace_resource_id,
        object_id: authority.object_id,
        object_generation: authority.object_generation,
        lease_generation: 1,
    };
    let instance = fixture.storage.identity().instance_id;
    let owner = identity.owner_key(instance);
    assert!(valid_opaque(&owner));
    assert_eq!(
        owner,
        ContainerLeaseIdentity {
            lease_generation: 2,
            ..identity
        }
        .owner_key(instance)
    );
    assert_ne!(owner, identity.owner_key(InstanceId::generate()));
    assert_ne!(
        owner,
        ContainerLeaseIdentity {
            namespace_resource_id: ResourceId::generate(),
            ..identity
        }
        .owner_key(instance)
    );
    assert_ne!(
        owner,
        ContainerLeaseIdentity {
            object_generation: 2,
            ..identity
        }
        .owner_key(instance)
    );
}

#[path = "containers_startup_tests.rs"]
mod startup_tests;

#[test]
fn application_delete_retains_lost_capacity_and_retires_only_after_positive_cleanup() {
    let fixture = fixture(4);
    let instance = fixture.authority.instance_id;
    let application = fixture.authority.application_id;
    let repo = ContainerAuthority::new(fixture.storage.db());
    let key = "6".repeat(64);
    let lease = repo
        .reserve(
            &fixture.authority,
            &key,
            &execution(&fixture.storage, &fixture.authority, &fixture.spec, &key),
            10,
            30,
        )
        .unwrap();
    repo.mark_starting(lease.identity, &key, 31).unwrap();
    repo.begin_application_delete(instance, application, 32)
        .unwrap();
    assert_eq!(
        repo.pending_application_deletions(instance).unwrap(),
        vec![application]
    );
    assert_eq!(
        repo.lease(instance, lease.identity)
            .unwrap()
            .stop_requested_at_ms,
        Some(32)
    );
    assert!(
        repo.finish_application_delete(instance, application, 33)
            .is_err()
    );
    repo.mark_lost(lease.identity, &key, 34).unwrap();
    assert!(repo.existing_lease(&fixture.authority).is_err());
    assert!(publish_target(repo, instance, application, &fixture.spec, b"{}", 35).is_err());
    assert_eq!(repo.applications(instance).unwrap().len(), 1);
    let path = fixture.directory.path().join("data");
    let spec = fixture.spec.clone();
    let namespace = fixture.authority.namespace_resource_id;
    drop(fixture.storage);
    let storage = PlatformStorage::bootstrap(&configuration(&path), &SystemClock).unwrap();
    let repo = ContainerAuthority::new(storage.db());
    assert_eq!(
        repo.pending_application_deletions(instance).unwrap(),
        vec![application]
    );
    assert_eq!(
        repo.lease(instance, lease.identity).unwrap().state,
        ContainerLeaseState::Lost
    );
    repo.finish_stopped(lease.identity, &termination(&key, None), 36)
        .unwrap();
    repo.finish_application_delete(instance, application, 37)
        .unwrap();
    assert!(repo.applications(instance).unwrap().is_empty());
    assert!(
        repo.pending_application_deletions(instance)
            .unwrap()
            .is_empty()
    );
    assert_eq!(repo.target(instance, application, 1).unwrap().spec, spec);
    assert_eq!(
        repo.application_leases(instance, application).unwrap()[0].state,
        ContainerLeaseState::Stopped
    );
    let fresh = ResourceId::generate();
    repo.create_application(instance, fresh, namespace, "container-app", 4, 38)
        .unwrap();
    assert_ne!(repo.applications(instance).unwrap()[0].id, application);
    assert!(publish_target(repo, instance, application, &spec, b"{}", 39).is_err());
}

#[test]
fn container_metadata_patch_preserves_current_configuration_and_omitted_fields() {
    let fixture = fixture(3);
    let instance = fixture.storage.identity().instance_id;
    let repo = ContainerAuthority::new(fixture.storage.db());
    let app = repo
        .application_for_namespace(
            instance,
            fixture.authority.namespace_resource_id,
            "container-app",
        )
        .unwrap();
    let metadata = ContainerApplicationMetadataPatch {
        max_instances: Some(3),
        observability_logs: Some(true),
    };
    repo.commit_application_configuration(
        instance,
        &app,
        &fixture.spec,
        b"{\"image\":\"first\"}",
        metadata,
        30,
    )
    .unwrap();
    // A stale metadata request must preserve a subsequently published configuration.
    let mut replacement = fixture.spec.clone();
    replacement.enable_internet = !replacement.enable_internet;
    let target = repo
        .commit_application_configuration(
            instance,
            &app,
            &replacement,
            b"{\"image\":\"second\"}",
            ContainerApplicationMetadataPatch {
                max_instances: Some(2),
                observability_logs: Some(false),
            },
            31,
        )
        .unwrap();
    assert_eq!(target.generation, 3);
    repo.patch_application_metadata(
        instance,
        app.id,
        ContainerApplicationMetadataPatch::default(),
        32,
    )
    .unwrap();
    assert_eq!(
        repo.application_settings(instance, app.id).unwrap(),
        (Some(b"{\"image\":\"second\"}".to_vec()), false, 32)
    );
    let current = repo
        .application_for_namespace(instance, app.namespace_resource_id, &app.name)
        .unwrap();
    assert_eq!(current.max_instances, 2);
    assert_eq!(current.target_generation, Some(3));
    repo.patch_application_metadata(
        instance,
        app.id,
        ContainerApplicationMetadataPatch {
            max_instances: None,
            observability_logs: Some(true),
        },
        33,
    )
    .unwrap();
    repo.patch_application_metadata(
        instance,
        app.id,
        ContainerApplicationMetadataPatch {
            max_instances: Some(1),
            observability_logs: None,
        },
        34,
    )
    .unwrap();
    assert_eq!(
        repo.application_settings(instance, app.id).unwrap(),
        (Some(b"{\"image\":\"second\"}".to_vec()), true, 34)
    );
    assert_eq!(
        repo.application_for_namespace(instance, app.namespace_resource_id, &app.name)
            .unwrap()
            .max_instances,
        1
    );
    // Configuration publication also merges omitted metadata against the current row.
    repo.commit_application_configuration(
        instance,
        &app,
        &replacement,
        b"{\"image\":\"second\"}",
        ContainerApplicationMetadataPatch::default(),
        35,
    )
    .unwrap();
    assert_eq!(
        repo.application_settings(instance, app.id).unwrap(),
        (Some(b"{\"image\":\"second\"}".to_vec()), true, 35)
    );
    assert_eq!(
        repo.application_for_namespace(instance, app.namespace_resource_id, &app.name)
            .unwrap()
            .max_instances,
        1
    );
    repo.begin_application_delete(instance, app.id, 36).unwrap();
    assert!(
        repo.patch_application_metadata(
            instance,
            app.id,
            ContainerApplicationMetadataPatch {
                max_instances: Some(2),
                observability_logs: Some(false)
            },
            37
        )
        .is_err()
    );
    assert_eq!(
        repo.application_settings(instance, app.id).unwrap(),
        (Some(b"{\"image\":\"second\"}".to_vec()), true, 35)
    );
}

#[path = "containers_rollout_tests.rs"]
mod rollout_target;

#[path = "containers_staging_tests.rs"]
mod staging_authority;

#[path = "containers_snapshot_tests.rs"]
mod snapshot_tests;

#[path = "containers_named_images_tests.rs"]
mod named_images;
