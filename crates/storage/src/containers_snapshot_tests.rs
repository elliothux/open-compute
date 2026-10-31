use super::*;

fn fixture(max_instances: u32) -> Fixture {
    fixture_with_policy(max_instances, true)
}

fn limits() -> ContainerSnapshotLimits {
    ContainerSnapshotLimits {
        max_snapshots: 4,
        max_bytes: 8192,
    }
}

fn source(fixture: &Fixture) -> ContainerSnapshotSource {
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
    ContainerSnapshotSource {
        identity: lease.identity,
        opaque_engine_key: key,
        start_identity: start,
    }
}

fn pending(fixture: &Fixture, source: &ContainerSnapshotSource, now: i64) -> ContainerSnapshot {
    ContainerAuthority::new(fixture.storage.db())
        .reserve_snapshot(
            &fixture.authority,
            &NewContainerSnapshot {
                directory: None,
                id: ResourceId::generate(),
                source: source.clone(),
                name: Some("daily".into()),
                reserved_bytes: 4096,
            },
            limits(),
            now,
        )
        .unwrap()
}

fn ready(fixture: &Fixture, source: &ContainerSnapshotSource) -> ContainerSnapshot {
    let repo = ContainerAuthority::new(fixture.storage.db());
    let snapshot = pending(fixture, source, 40);
    repo.stage_snapshot_artifact(
        fixture.authority.instance_id,
        snapshot.id,
        source,
        &image(21),
    )
    .unwrap();
    repo.finish_snapshot_capture(&fixture.authority, snapshot.id, source, 1502, 41)
        .unwrap()
}

fn other_object(fixture: &Fixture, fill: u8) -> ContainerObjectAuthority {
    ContainerObjectAuthority {
        object_id: insert_object(
            &fixture.storage,
            fixture.authority.namespace_resource_id,
            fill,
        ),
        ..fixture.authority
    }
}

fn restore(
    fixture: &Fixture,
    authority: &ContainerObjectAuthority,
    snapshot: ResourceId,
    key: &str,
) -> ContainerExecutionSpec {
    let mut spec = execution(&fixture.storage, authority, &fixture.spec, key);
    spec.image_sha256 = image(21).manifest_sha256;
    spec.startup.snapshot_id = Some(snapshot);
    spec.startup.image_reference.clear();
    spec
}

#[test]
fn directory_snapshot_authority_is_durable_application_scoped_and_not_a_boot_image() {
    let fixture = fixture(4);
    let repo = ContainerAuthority::new(fixture.storage.db());
    let source = source(&fixture);
    let directory =
        open_compute_core::containers::ContainerDirectoryPath::parse("/app/data").unwrap();
    let pending = repo
        .reserve_snapshot(
            &fixture.authority,
            &NewContainerSnapshot {
                id: ResourceId::generate(),
                source: source.clone(),
                name: Some("data".into()),
                directory: Some(directory.clone()),
                reserved_bytes: 4096,
            },
            limits(),
            40,
        )
        .unwrap();
    assert!(
        repo.directory_snapshot_for_restore(&fixture.authority, pending.id, 40)
            .is_err()
    );
    repo.stage_snapshot_artifact(
        fixture.authority.instance_id,
        pending.id,
        &source,
        &image(21),
    )
    .unwrap();
    let snapshot = repo
        .finish_snapshot_capture(&fixture.authority, pending.id, &source, 1502, 41)
        .unwrap();
    let other = other_object(&fixture, 19);
    assert_eq!(
        repo.directory_snapshot_for_restore(&other, snapshot.id, 42)
            .unwrap(),
        (snapshot.clone(), image(21))
    );
    assert!(repo.snapshot_for_restore(&other, snapshot.id, 42).is_err());
    assert!(
        repo.reserve(
            &other,
            &"c".repeat(64),
            &restore(&fixture, &other, snapshot.id, &"c".repeat(64)),
            10,
            43
        )
        .is_err()
    );
    for invalid in [
        ContainerObjectAuthority {
            instance_id: InstanceId::generate(),
            ..other
        },
        ContainerObjectAuthority {
            application_id: ResourceId::generate(),
            ..other
        },
        ContainerObjectAuthority {
            route_generation: other.route_generation + 1,
            ..other
        },
    ] {
        assert!(
            repo.directory_snapshot_for_restore(&invalid, snapshot.id, 42)
                .is_err()
        );
    }
    assert!(
        repo.directory_snapshot_for_restore(&other, snapshot.id, snapshot.expires_at_ms)
            .is_err()
    );
    fixture
        .storage
        .db()
        .with_immediate(|tx| {
            for value in [None, Some("/other"), Some("/app/../data")] {
                assert!(
                    tx.execute(
                        "UPDATE container_snapshots SET directory_path=?2 WHERE id=?1",
                        params![snapshot.id, value]
                    )
                    .is_err()
                );
            }
            Ok(())
        })
        .unwrap();
    let data = fixture.directory.path().join("data");
    drop(fixture.storage);
    let storage = PlatformStorage::bootstrap(&configuration(&data), &SystemClock).unwrap();
    let reopened = ContainerAuthority::new(storage.db());
    assert_eq!(
        reopened
            .directory_snapshot_for_restore(&other, snapshot.id, 42)
            .unwrap(),
        (snapshot, image(21))
    );
    assert!(
        reopened
            .snapshot_for_restore(&other, pending.id, 42)
            .is_err()
    );
}

#[test]
fn whole_snapshot_cannot_supply_directory_restore_authority() {
    let fixture = fixture(4);
    let source = source(&fixture);
    let snapshot = ready(&fixture, &source);
    let repo = ContainerAuthority::new(fixture.storage.db());
    assert!(
        repo.directory_snapshot_for_restore(&fixture.authority, snapshot.id, 42)
            .is_err()
    );
    assert_eq!(
        repo.snapshot_for_restore(&fixture.authority, snapshot.id, 42)
            .unwrap(),
        image(21)
    );
}

#[test]
fn snapshot_publication_is_fenced_and_restart_preserves_unknown_artifact_identity() {
    let fixture = fixture(4);
    let repo = ContainerAuthority::new(fixture.storage.db());
    let instance = fixture.authority.instance_id;
    let source = source(&fixture);
    let snapshot = pending(&fixture, &source, 40);
    assert!(
        repo.snapshot_for_restore(&fixture.authority, snapshot.id, 41)
            .is_err()
    );
    assert!(
        repo.finish_snapshot_capture(&fixture.authority, snapshot.id, &source, 1502, 41)
            .is_err()
    );
    let mut wrong = source.clone();
    wrong.start_identity = "c".repeat(64);
    assert!(
        repo.stage_snapshot_artifact(instance, snapshot.id, &wrong, &image(21))
            .is_err()
    );
    let mut oversized = image(21);
    oversized.artifact_size = 4097;
    assert!(
        repo.stage_snapshot_artifact(instance, snapshot.id, &source, &oversized)
            .is_err()
    );
    let mut wrong_architecture = image(21);
    wrong_architecture.architecture = "amd64".into();
    assert!(
        repo.stage_snapshot_artifact(instance, snapshot.id, &source, &wrong_architecture)
            .is_err()
    );
    repo.stage_snapshot_artifact(instance, snapshot.id, &source, &image(21))
        .unwrap();
    assert!(
        WorkerRepository::new(fixture.storage.db())
            .referenced_artifacts()
            .unwrap()
            .contains(&(image(21).artifact_sha256, 2048))
    );
    let expected = repo.snapshots(instance).unwrap();
    let data = fixture.directory.path().join("data");
    drop(fixture.storage);
    let storage = PlatformStorage::bootstrap(&configuration(&data), &SystemClock).unwrap();
    let repo = ContainerAuthority::new(storage.db());
    assert_eq!(repo.snapshots(instance).unwrap(), expected);
    let mut stale = fixture.authority;
    stale.route_generation += 1;
    assert!(
        repo.finish_snapshot_capture(&stale, snapshot.id, &source, 1502, 42)
            .is_err()
    );
    repo.begin_stop(source.identity, &source.opaque_engine_key, 43)
        .unwrap();
    assert!(
        repo.finish_snapshot_capture(&fixture.authority, snapshot.id, &source, 1502, 44)
            .is_err()
    );
    assert!(repo.retire_snapshot(instance, snapshot.id).unwrap());
    assert!(!repo.retire_snapshot(instance, snapshot.id).unwrap());
    assert!(
        repo.finish_snapshot_retirement(instance, snapshot.id, true)
            .is_err()
    );
    assert!(
        WorkerRepository::new(storage.db())
            .referenced_artifacts()
            .unwrap()
            .contains(&(image(21).artifact_sha256, 2048))
    );
    repo.finish_stopped(
        source.identity,
        &termination(&source.opaque_engine_key, Some(&source.start_identity)),
        45,
    )
    .unwrap();
    assert!(
        !WorkerRepository::new(storage.db())
            .referenced_artifacts()
            .unwrap()
            .contains(&(image(21).artifact_sha256, 2048))
    );
    assert!(
        repo.finish_snapshot_retirement(instance, snapshot.id, false)
            .is_err()
    );
    repo.finish_snapshot_retirement(instance, snapshot.id, true)
        .unwrap();
    assert!(repo.snapshots(instance).unwrap().is_empty());
}

#[test]
fn snapshot_acl_is_application_scoped_and_cannot_be_bypassed_by_image_digest() {
    let fixture = fixture(4);
    let repo = ContainerAuthority::new(fixture.storage.db());
    let source = source(&fixture);
    let snapshot = ready(&fixture, &source);
    let other = other_object(&fixture, 19);
    assert_eq!(
        repo.snapshot_for_restore(&other, snapshot.id, 42).unwrap(),
        image(21)
    );
    assert!(
        repo.ordinary_image(other.instance_id, &image(21).manifest_sha256)
            .is_err()
    );
    assert!(repo.admit_image(other.instance_id, &image(21)).is_err());
    let key = "c".repeat(64);
    let mut spec = restore(&fixture, &other, snapshot.id, &key);
    spec.startup.snapshot_id = None;
    spec.startup.image_reference =
        format!("registry/derived@sha256:{}", hex::encode(spec.image_sha256));
    assert!(repo.reserve(&other, &key, &spec, 10, 43).is_err());
    spec.startup.snapshot_id = Some(snapshot.id);
    spec.startup.image_reference.clear();
    spec.startup.image_override = true;
    assert!(repo.reserve(&other, &key, &spec, 10, 43).is_err());
    spec.startup.image_override = false;
    spec.startup.image_reference =
        format!("registry/app@sha256:{}", hex::encode(spec.image_sha256));
    assert!(repo.reserve(&other, &key, &spec, 10, 43).is_err());
    spec.startup.image_reference.clear();
    spec.image_sha256 = fixture.spec.image_sha256.unwrap();
    assert!(repo.reserve(&other, &key, &spec, 10, 43).is_err());
    let mut invalid = other;
    invalid.instance_id = InstanceId::generate();
    assert!(
        repo.snapshot_for_restore(&invalid, snapshot.id, 42)
            .is_err()
    );
    invalid = other;
    invalid.route_generation += 1;
    assert!(
        repo.snapshot_for_restore(&invalid, snapshot.id, 42)
            .is_err()
    );
    invalid = other;
    invalid.object_generation += 1;
    assert!(
        repo.snapshot_for_restore(&invalid, snapshot.id, 42)
            .is_err()
    );
    invalid = other;
    invalid.application_id = ResourceId::generate();
    assert!(
        repo.snapshot_for_restore(&invalid, snapshot.id, 42)
            .is_err()
    );
    let mut target = fixture.spec.clone();
    target.image_sha256 = Some(image(21).manifest_sha256);
    assert!(
        publish_target(
            repo,
            other.instance_id,
            other.application_id,
            &target,
            b"{\"image\":\"private\"}",
            43
        )
        .is_err()
    );
    let repository = format!("{}/app", other.instance_id);
    assert!(
        repo.publish_registry_tag(
            other.instance_id,
            &repository,
            "snapshot",
            &image(21).manifest_sha256,
            43
        )
        .is_err()
    );
    // Source object deletion does not revoke the application's already published snapshot.
    repo.finish_stopped(
        source.identity,
        &termination(&source.opaque_engine_key, Some(&source.start_identity)),
        44,
    )
    .unwrap();
    fixture.storage.db().with_immediate(|tx| {
        tx.execute("UPDATE do_objects SET state='deleting' WHERE namespace_resource_id=?1 AND object_id=?2",params![source.identity.namespace_resource_id,source.identity.object_id.to_string()]).unwrap();
        tx.execute("UPDATE do_objects SET state='tombstoned',deleted_at_ms=44 WHERE namespace_resource_id=?1 AND object_id=?2",params![source.identity.namespace_resource_id,source.identity.object_id.to_string()]).unwrap();
        Ok(())
    }).unwrap();
    assert!(repo.snapshot_for_restore(&other, snapshot.id, 45).is_ok());
    repo.begin_application_delete(other.instance_id, other.application_id, 46)
        .unwrap();
    assert!(repo.snapshot_for_restore(&other, snapshot.id, 47).is_err());
    assert_eq!(
        repo.snapshots(other.instance_id).unwrap()[0].state,
        ContainerSnapshotState::Retiring
    );
    assert!(
        repo.finish_application_delete(other.instance_id, other.application_id, 48)
            .is_err()
    );
    repo.finish_snapshot_retirement(other.instance_id, snapshot.id, true)
        .unwrap();
    repo.finish_application_delete(other.instance_id, other.application_id, 48)
        .unwrap();
    let recreated = ResourceId::generate();
    repo.create_application(
        other.instance_id,
        recreated,
        other.namespace_resource_id,
        "container-app",
        4,
        49,
    )
    .unwrap();
    publish_target(repo, other.instance_id, recreated, &fixture.spec, b"{}", 50).unwrap();
    let new_authority = ContainerObjectAuthority {
        application_id: recreated,
        ..other
    };
    assert!(repo.existing_lease(&new_authority).unwrap().is_none());
    assert!(
        repo.snapshot_for_restore(&new_authority, snapshot.id, 51)
            .is_err()
    );
}

#[test]
fn first_positive_restore_renews_once_and_pins_expired_artifact_through_uncertainty() {
    let fixture = fixture(4);
    let repo = ContainerAuthority::new(fixture.storage.db());
    let source = source(&fixture);
    let snapshot = ready(&fixture, &source);
    let other = other_object(&fixture, 19);
    let key = "c".repeat(64);
    let spec = restore(&fixture, &other, snapshot.id, &key);
    let lease = repo
        .reserve(&other, &key, &spec, 10, snapshot.expires_at_ms - 1)
        .unwrap();
    assert_eq!(
        repo.snapshots(other.instance_id).unwrap()[0].expires_at_ms,
        snapshot.expires_at_ms
    );
    assert_eq!(
        repo.retire_snapshots(other.instance_id, snapshot.expires_at_ms)
            .unwrap(),
        0
    );
    let next = other_object(&fixture, 20);
    let next_spec = restore(&fixture, &next, snapshot.id, &"d".repeat(64));
    assert!(
        repo.reserve(
            &next,
            &"d".repeat(64),
            &next_spec,
            10,
            snapshot.expires_at_ms
        )
        .is_err()
    );
    repo.mark_starting(lease.identity, &key, snapshot.expires_at_ms)
        .unwrap();
    repo.mark_lost(lease.identity, &key, snapshot.expires_at_ms + 1)
        .unwrap();
    let observation = ContainerRunningObservation {
        opaque_engine_key: &key,
        start_identity: &"e".repeat(64),
        image_sha256: &spec.image_sha256,
        provider_contract_sha256: &fixture.spec.provider_contract_sha256,
    };
    let mut wrong = observation;
    wrong.image_sha256 = fixture.spec.image_sha256.as_ref().unwrap();
    assert!(
        repo.observe_running(lease.identity, &wrong, snapshot.expires_at_ms + 2)
            .is_err()
    );
    assert_eq!(
        repo.snapshots(other.instance_id).unwrap()[0].last_restored_at_ms,
        None
    );
    let positive = snapshot.expires_at_ms + 3;
    repo.observe_running(lease.identity, &observation, positive)
        .unwrap();
    let renewed = repo.snapshots(other.instance_id).unwrap().remove(0);
    assert_eq!(
        renewed.expires_at_ms,
        positive + CONTAINER_SNAPSHOT_RETENTION_MS
    );
    assert_eq!(renewed.last_restored_at_ms, Some(positive));
    repo.observe_running(lease.identity, &observation, positive + 1000)
        .unwrap();
    assert_eq!(repo.snapshots(other.instance_id).unwrap()[0], renewed);
    repo.begin_application_delete(other.instance_id, other.application_id, positive + 1000)
        .unwrap();
    assert!(
        WorkerRepository::new(fixture.storage.db())
            .referenced_artifacts()
            .unwrap()
            .contains(&(image(21).artifact_sha256, 2048))
    );
    assert!(
        repo.finish_snapshot_retirement(other.instance_id, snapshot.id, true)
            .is_err()
    );
    repo.mark_lost(lease.identity, &key, positive + 1001)
        .unwrap();
    assert!(
        repo.finish_snapshot_retirement(other.instance_id, snapshot.id, true)
            .is_err()
    );
    repo.finish_stopped(
        lease.identity,
        &termination(&key, Some(observation.start_identity)),
        positive + 1002,
    )
    .unwrap();
    assert!(
        !WorkerRepository::new(fixture.storage.db())
            .referenced_artifacts()
            .unwrap()
            .contains(&(image(21).artifact_sha256, 2048))
    );
    repo.finish_snapshot_retirement(other.instance_id, snapshot.id, true)
        .unwrap();
}

#[test]
fn pending_ready_and_retiring_charges_require_positive_absence_before_reuse() {
    let fixture = fixture(4);
    let repo = ContainerAuthority::new(fixture.storage.db());
    let source = source(&fixture);
    let instance = fixture.authority.instance_id;
    let snapshot = pending(&fixture, &source, 40);
    let request = NewContainerSnapshot {
        directory: None,
        id: ResourceId::generate(),
        source: source.clone(),
        name: None,
        reserved_bytes: 1,
    };
    let count_limit = ContainerSnapshotLimits {
        max_snapshots: 1,
        max_bytes: 8192,
    };
    assert_eq!(
        repo.reserve_snapshot(&fixture.authority, &request, count_limit, 41)
            .unwrap_err()
            .code(),
        ErrorCode::AdmissionBusy
    );
    let byte_limit = ContainerSnapshotLimits {
        max_snapshots: 4,
        max_bytes: 4096,
    };
    assert_eq!(
        repo.reserve_snapshot(&fixture.authority, &request, byte_limit, 41)
            .unwrap_err()
            .code(),
        ErrorCode::AdmissionBusy
    );
    repo.stage_snapshot_artifact(instance, snapshot.id, &source, &image(21))
        .unwrap();
    assert_eq!(
        repo.reserve_snapshot(&fixture.authority, &request, byte_limit, 41)
            .unwrap_err()
            .code(),
        ErrorCode::AdmissionBusy
    );
    repo.finish_snapshot_capture(&fixture.authority, snapshot.id, &source, 1502, 42)
        .unwrap();
    // Positive publication releases unused reservation, but not the retained artifact.
    let exact = ContainerSnapshotLimits {
        max_snapshots: 4,
        max_bytes: 2049,
    };
    repo.reserve_snapshot(&fixture.authority, &request, exact, 43)
        .unwrap();
    assert!(!repo.retire_snapshot(instance, snapshot.id).unwrap());
    assert_eq!(
        repo.lease(instance, source.identity).unwrap().state,
        ContainerLeaseState::Running
    );
    assert_eq!(
        repo.retire_snapshots(instance, 42 + CONTAINER_SNAPSHOT_RETENTION_MS)
            .unwrap(),
        1
    );
    let another = NewContainerSnapshot {
        directory: None,
        id: ResourceId::generate(),
        ..request
    };
    assert!(
        repo.reserve_snapshot(
            &fixture.authority,
            &another,
            byte_limit,
            42 + CONTAINER_SNAPSHOT_RETENTION_MS + 1
        )
        .is_err()
    );
    assert!(
        repo.finish_snapshot_retirement(instance, snapshot.id, false)
            .is_err()
    );
    repo.finish_snapshot_retirement(instance, snapshot.id, true)
        .unwrap();
    repo.reserve_snapshot(
        &fixture.authority,
        &another,
        byte_limit,
        42 + CONTAINER_SNAPSHOT_RETENTION_MS + 2,
    )
    .unwrap();
}

#[test]
fn snapshot_expiry_is_exclusive_and_derived_lineage_stays_bound_to_the_application_image() {
    let fixture = fixture(4);
    let repo = ContainerAuthority::new(fixture.storage.db());
    let source = source(&fixture);
    let snapshot = ready(&fixture, &source);
    assert_eq!(snapshot.captured_at_ms, Some(41));
    assert_eq!(snapshot.expires_at_ms, 41 + CONTAINER_SNAPSHOT_RETENTION_MS);
    assert!(
        repo.snapshot_for_restore(&fixture.authority, snapshot.id, snapshot.expires_at_ms - 1)
            .is_ok()
    );
    assert!(
        repo.snapshot_for_restore(&fixture.authority, snapshot.id, snapshot.expires_at_ms)
            .is_err()
    );
    let other = other_object(&fixture, 19);
    let key = "c".repeat(64);
    let spec = restore(&fixture, &other, snapshot.id, &key);
    let lease = repo.reserve(&other, &key, &spec, 10, 42).unwrap();
    repo.mark_starting(lease.identity, &key, 43).unwrap();
    let start = "d".repeat(64);
    repo.observe_running(
        lease.identity,
        &ContainerRunningObservation {
            opaque_engine_key: &key,
            start_identity: &start,
            image_sha256: &spec.image_sha256,
            provider_contract_sha256: &fixture.spec.provider_contract_sha256,
        },
        44,
    )
    .unwrap();
    let restored_source = ContainerSnapshotSource {
        identity: lease.identity,
        opaque_engine_key: key.clone(),
        start_identity: start,
    };
    let second = repo
        .reserve_snapshot(
            &other,
            &NewContainerSnapshot {
                directory: None,
                id: ResourceId::generate(),
                source: restored_source.clone(),
                name: None,
                reserved_bytes: 4096,
            },
            limits(),
            45,
        )
        .unwrap();
    assert_eq!(
        second.source_image_sha256,
        fixture.spec.image_sha256.unwrap()
    );
    repo.stage_snapshot_artifact(other.instance_id, second.id, &restored_source, &image(31))
        .unwrap();
    repo.finish_snapshot_capture(&other, second.id, &restored_source, 1502, 46)
        .unwrap();
    let third = other_object(&fixture, 20);
    assert!(repo.snapshot_for_restore(&third, second.id, 47).is_ok());
    repo.admit_image(other.instance_id, &image(41)).unwrap();
    // Admitting another image does not change this DO application's snapshot authority.
    assert!(repo.snapshot_for_restore(&third, snapshot.id, 49).is_ok());
    assert!(repo.snapshot_for_restore(&third, second.id, 49).is_ok());
    assert_eq!(
        repo.retire_snapshots(other.instance_id, snapshot.expires_at_ms + 100)
            .unwrap(),
        1
    );
}

#[test]
fn snapshot_restore_and_retirement_serialize_without_unpinning_a_reserved_lease() {
    use std::sync::{Arc, Barrier};
    let fixture = fixture(4);
    let repo = ContainerAuthority::new(fixture.storage.db());
    let source = source(&fixture);
    let snapshot = ready(&fixture, &source);
    let other = other_object(&fixture, 19);
    let key = "c".repeat(64);
    let spec = restore(&fixture, &other, snapshot.id, &key);
    let data = fixture.directory.path().join("data/control.sqlite");
    let barrier = Arc::new(Barrier::new(2));
    std::thread::scope(|scope| {
        let barrier2 = barrier.clone();
        let data2 = data.clone();
        let reservation = scope.spawn(move || {
            let db = ControlDb::open(&data2, 5000).unwrap();
            barrier2.wait();
            ContainerAuthority::new(&db).reserve(&other, &key, &spec, 10, 42)
        });
        let db = ControlDb::open(&data, 5000).unwrap();
        barrier.wait();
        ContainerAuthority::new(&db)
            .retire_snapshots(other.instance_id, snapshot.expires_at_ms)
            .unwrap();
        let lease = reservation.join().unwrap();
        let refs = WorkerRepository::new(fixture.storage.db())
            .referenced_artifacts()
            .unwrap();
        if let Ok(lease) = lease {
            assert!(refs.contains(&(image(21).artifact_sha256, 2048)));
            assert!(
                repo.finish_snapshot_retirement(other.instance_id, snapshot.id, true)
                    .is_err()
            );
            repo.finish_stopped(
                lease.identity,
                &termination(&lease.opaque_engine_key, None),
                43,
            )
            .unwrap();
        } else {
            assert!(repo.existing_lease(&other).unwrap().is_none());
            assert!(!refs.contains(&(image(21).artifact_sha256, 2048)));
        }
        repo.retire_snapshots(other.instance_id, snapshot.expires_at_ms)
            .unwrap();
        repo.finish_snapshot_retirement(other.instance_id, snapshot.id, true)
            .unwrap();
    });
}

#[test]
fn snapshot_reservation_rejects_forged_source_names_and_arithmetic_overflow() {
    let fixture = fixture(4);
    let repo = ContainerAuthority::new(fixture.storage.db());
    let source = source(&fixture);
    let input = NewContainerSnapshot {
        directory: None,
        id: ResourceId::generate(),
        source: source.clone(),
        name: None,
        reserved_bytes: 4096,
    };
    for reserved_bytes in [0, u64::MAX] {
        let input = NewContainerSnapshot {
            directory: None,
            reserved_bytes,
            ..input.clone()
        };
        assert!(
            repo.reserve_snapshot(&fixture.authority, &input, limits(), 40)
                .is_err()
        );
    }
    for name in ["x".repeat(257), "hidden\0tail".into()] {
        let input = NewContainerSnapshot {
            directory: None,
            name: Some(name),
            ..input.clone()
        };
        assert!(
            repo.reserve_snapshot(&fixture.authority, &input, limits(), 40)
                .is_err()
        );
    }
    assert!(
        repo.reserve_snapshot(&fixture.authority, &input, limits(), i64::MAX)
            .is_err()
    );
    let mut forged = input.clone();
    forged.source.opaque_engine_key = "e".repeat(64);
    assert!(
        repo.reserve_snapshot(&fixture.authority, &forged, limits(), 40)
            .is_err()
    );
    forged = input.clone();
    forged.source.identity.lease_generation += 1;
    assert!(
        repo.reserve_snapshot(&fixture.authority, &forged, limits(), 40)
            .is_err()
    );
    assert!(
        repo.snapshots(fixture.authority.instance_id)
            .unwrap()
            .is_empty()
    );
    assert!(
        serde_json::from_slice::<ContainerLeaseStartup>(
            br#"{"imageReference":"","instanceProfile":null,"imageOverride":false,"resourcesOverride":false}"#
        )
        .is_err()
    );
}

#[test]
fn snapshot_artifact_identity_and_restore_metadata_are_immutable() {
    let fixture = fixture(4);
    let repo = ContainerAuthority::new(fixture.storage.db());
    let source = source(&fixture);
    let snapshot = pending(&fixture, &source, 40);
    let instance = fixture.authority.instance_id;
    repo.stage_snapshot_artifact(instance, snapshot.id, &source, &image(21))
        .unwrap();
    repo.stage_snapshot_artifact(instance, snapshot.id, &source, &image(21))
        .unwrap();
    assert!(
        repo.stage_snapshot_artifact(instance, snapshot.id, &source, &image(31))
            .is_err()
    );
    assert!(repo.image(instance, &image(31).manifest_sha256).is_err());
    repo.finish_snapshot_capture(&fixture.authority, snapshot.id, &source, 1502, 41)
        .unwrap();
    let other = other_object(&fixture, 19);
    let key = "c".repeat(64);
    let spec = restore(&fixture, &other, snapshot.id, &key);
    let lease = repo.reserve(&other, &key, &spec, 10, 42).unwrap();
    let mut changed = spec.clone();
    changed.startup.snapshot_id = None;
    assert!(repo.reserve(&other, &key, &changed, 10, 43).is_err());
    fixture.storage.db().with_immediate(|tx| {
        assert!(tx.execute("UPDATE container_instance_leases SET snapshot_id=NULL WHERE opaque_engine_key=?1",[&key]).is_err());
        assert!(tx.execute("UPDATE container_snapshots SET artifact_size=1024 WHERE id=?1",[snapshot.id]).is_err());
        assert!(tx.execute("UPDATE container_images SET snapshot_only=0 WHERE manifest_sha256=?1",[image(21).manifest_sha256]).is_err());
        Ok(())
    }).unwrap();
    assert_eq!(
        repo.lease(instance, lease.identity)
            .unwrap()
            .startup
            .snapshot_id,
        Some(snapshot.id)
    );
}

#[test]
fn v17_appends_snapshot_authority_without_changing_v16_history_or_regular_images() {
    use crate::schema_migrations::{self, DatabaseKind};
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("control.sqlite");
    let mut connection = Connection::open(&path).unwrap();
    connection
        .pragma_update(None, "foreign_keys", "ON")
        .unwrap();
    schema_migrations::migrate_to_for_test(&mut connection, DatabaseKind::Control, 16);
    let original: String = connection
        .query_row(
            "SELECT checksum FROM refinery_schema_history WHERE version=16",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let regular = image(1);
    connection.execute("INSERT INTO container_images(manifest_sha256,config_sha256,diff_ids,architecture,compressed_bytes,unpacked_bytes,artifact_sha256,artifact_size,created_at_ms) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",params![regular.manifest_sha256,regular.config_sha256,regular.diff_ids.iter().flatten().copied().collect::<Vec<_>>(),regular.architecture,regular.compressed_bytes,regular.unpacked_bytes,regular.artifact_sha256,regular.artifact_size,regular.created_at_ms]).unwrap();
    drop(connection);
    let db = ControlDb::open(&path, 100).unwrap();
    crate::migrations::apply(&db, &SystemClock).unwrap();
    assert_eq!(
        crate::migrations::inspect_schema(&db).unwrap(),
        crate::migrations::current_schema_version()
    );
    db.with_read(|conn| {
        let after:String=conn.query_row("SELECT checksum FROM refinery_schema_history WHERE version=16",[],|row|row.get(0)).unwrap();
        assert_eq!(after,original);
        assert_eq!(conn.query_row("SELECT snapshot_only FROM container_images",[],|row|row.get::<_,i64>(0)).unwrap(),0);
        assert_eq!(conn.query_row("SELECT COUNT(*) FROM container_snapshots",[],|row|row.get::<_,i64>(0)).unwrap(),0);
        assert_eq!(conn.query_row("SELECT COUNT(*) FROM pragma_table_info('container_instance_leases') WHERE name='snapshot_id'",[],|row|row.get::<_,i64>(0)).unwrap(),1);
        assert_eq!(conn.query_row("SELECT COUNT(*) FROM pragma_table_info('container_snapshots') WHERE name='image_size' AND type='INTEGER' AND [notnull]=0",[],|row|row.get::<_,i64>(0)).unwrap(),1);
        Ok(())
    }).unwrap();
    db.with_immediate(|tx| {
        tx.execute(
            "UPDATE refinery_schema_history SET checksum='1' WHERE version=17",
            [],
        )
        .unwrap();
        Ok(())
    })
    .unwrap();
    assert_eq!(
        crate::migrations::apply(&db, &SystemClock)
            .unwrap_err()
            .code(),
        ErrorCode::MigrationFailed
    );
}

fn another_application(fixture: &Fixture) -> ContainerObjectAuthority {
    let instance = fixture.authority.instance_id;
    let (worker, _) = WorkerRepository::new(fixture.storage.db())
        .create_worker(
            instance,
            "other-container",
            RequestId::generate(),
            50,
            1_000_000,
        )
        .unwrap();
    let namespace = ResourceId::generate();
    let resources = ResourceRepository::new(fixture.storage.db());
    let resource = match resources
        .reserve_create(
            &ReserveResourceCreate {
                instance_id: instance,
                kind: BindingKind::DoNamespace,
                name: "OTHER_CONTAINER",
                idempotency_key: "other-container",
                fingerprint_key_id: "key",
                request_fingerprint: &[4; 32],
                resource_id: namespace,
                driver_schema_version: DO_NAMESPACE_SCHEMA_VERSION,
                request_id: RequestId::generate(),
                now_ms: 51,
                expires_at_ms: 1000,
            },
            1_000_000,
        )
        .unwrap()
    {
        ResourceCreateReservation::Reserved(resource) => resource,
        other => panic!("unexpected {other:?}"),
    };
    DurableObjectRepository::new(&fixture.storage)
        .ensure_namespace(&resource, worker.id, "OtherContainer")
        .unwrap();
    resources.mark_ready(namespace, 52).unwrap();
    let version = version(&fixture.storage, worker.id);
    let repo = ContainerAuthority::new(fixture.storage.db());
    repo.declare_version(
        instance,
        version,
        namespace,
        "other-app",
        &declaration_digest(namespace, "other-app"),
    )
    .unwrap();
    let application = namespace;
    let route = activate(&fixture.storage, worker.id, version);
    repo.create_durable_object_application(
        instance,
        &ContainerApplication {
            id: application,
            namespace_resource_id: namespace,
            name: "other-app".into(),
            scheduling_policy: ContainerSchedulingPolicy::DurableObject,
            max_instances: 4,
            target_generation: None,
            admission_paused: true,
            created_at_ms: 53,
        },
        &ContainerTargetSpec {
            worker_version_id: version,
            image_sha256: None,
            ..fixture.spec.clone()
        },
        false,
        54,
    )
    .unwrap();
    ContainerObjectAuthority {
        instance_id: instance,
        application_id: application,
        namespace_resource_id: namespace,
        object_id: insert_object(&fixture.storage, namespace, 31),
        object_generation: 1,
        route_generation: route,
        descriptor_sha256: declaration_digest(namespace, "other-app"),
    }
}

#[test]
fn a_live_unrelated_application_cannot_restore_or_reserve_another_applications_snapshot() {
    let fixture = fixture(4);
    let repo = ContainerAuthority::new(fixture.storage.db());
    let source = source(&fixture);
    let snapshot = ready(&fixture, &source);
    let unrelated = another_application(&fixture);
    assert!(repo.existing_lease(&unrelated).unwrap().is_none());
    assert_eq!(
        repo.snapshot_for_restore(&unrelated, snapshot.id, 60)
            .unwrap_err()
            .code(),
        ErrorCode::ResourceNotFound
    );
    let spec = restore(&fixture, &unrelated, snapshot.id, &"e".repeat(64));
    assert_eq!(
        repo.reserve(&unrelated, &"e".repeat(64), &spec, 10, 60)
            .unwrap_err()
            .code(),
        ErrorCode::ResourceNotFound
    );
    assert!(repo.existing_lease(&unrelated).unwrap().is_none());
    assert_eq!(repo.snapshots(unrelated.instance_id).unwrap()[0], snapshot);
}

#[test]
fn restore_reservation_and_first_running_retention_survive_daemon_restart() {
    let fixture = fixture(4);
    let repo = ContainerAuthority::new(fixture.storage.db());
    let source = source(&fixture);
    let snapshot = ready(&fixture, &source);
    let other = other_object(&fixture, 19);
    let key = "c".repeat(64);
    let spec = restore(&fixture, &other, snapshot.id, &key);
    let lease = repo.reserve(&other, &key, &spec, 10, 42).unwrap();
    repo.mark_starting(lease.identity, &key, 43).unwrap();
    let data = fixture.directory.path().join("data");
    drop(fixture.storage);
    let storage = PlatformStorage::bootstrap(&configuration(&data), &SystemClock).unwrap();
    let repo = ContainerAuthority::new(storage.db());
    repo.lose_all(other.instance_id, 44).unwrap();
    assert!(
        WorkerRepository::new(storage.db())
            .referenced_artifacts()
            .unwrap()
            .contains(&(image(21).artifact_sha256, 2048))
    );
    let now = snapshot.expires_at_ms + 1;
    let start = "d".repeat(64);
    repo.observe_running(
        lease.identity,
        &ContainerRunningObservation {
            opaque_engine_key: &key,
            start_identity: &start,
            image_sha256: &spec.image_sha256,
            provider_contract_sha256: &fixture.spec.provider_contract_sha256,
        },
        now,
    )
    .unwrap();
    let renewed = repo.snapshots(other.instance_id).unwrap();
    assert_eq!(
        renewed[0].expires_at_ms,
        now + CONTAINER_SNAPSHOT_RETENTION_MS
    );
    drop(storage);
    let storage = PlatformStorage::bootstrap(&configuration(&data), &SystemClock).unwrap();
    let repo = ContainerAuthority::new(storage.db());
    repo.observe_running(
        lease.identity,
        &ContainerRunningObservation {
            opaque_engine_key: &key,
            start_identity: &start,
            image_sha256: &spec.image_sha256,
            provider_contract_sha256: &fixture.spec.provider_contract_sha256,
        },
        now + 1,
    )
    .unwrap();
    assert_eq!(repo.snapshots(other.instance_id).unwrap(), renewed);
}

#[test]
fn unknown_put_is_fenced_before_io_and_never_releases_its_charge_after_restart() {
    let fixture = fixture(4);
    let repo = ContainerAuthority::new(fixture.storage.db());
    let instance = fixture.authority.instance_id;
    let source = source(&fixture);
    let snapshot = pending(&fixture, &source, 40);
    assert!(!snapshot.put_attempted);
    assert!(
        repo.mark_snapshot_put_attempted(instance, snapshot.id, &source)
            .is_err()
    );
    repo.stage_snapshot_artifact(instance, snapshot.id, &source, &image(21))
        .unwrap();
    let mut wrong = source.clone();
    wrong.start_identity = "c".repeat(64);
    assert!(
        repo.mark_snapshot_put_attempted(instance, snapshot.id, &wrong)
            .is_err()
    );
    wrong = source.clone();
    wrong.identity.lease_generation += 1;
    assert!(
        repo.mark_snapshot_put_attempted(instance, snapshot.id, &wrong)
            .is_err()
    );
    assert!(
        repo.mark_snapshot_put_attempted(InstanceId::generate(), snapshot.id, &source)
            .is_err()
    );
    assert!(!repo.snapshots(instance).unwrap()[0].put_attempted);
    repo.mark_snapshot_put_attempted(instance, snapshot.id, &source)
        .unwrap();
    repo.mark_snapshot_put_attempted(instance, snapshot.id, &source)
        .unwrap();
    fixture
        .storage
        .db()
        .with_immediate(|tx| {
            assert!(
                tx.execute(
                    "UPDATE container_snapshots SET put_attempted=0 WHERE id=?1",
                    [snapshot.id]
                )
                .is_err()
            );
            Ok(())
        })
        .unwrap();
    assert!(repo.retire_snapshot(instance, snapshot.id).unwrap());
    assert!(
        repo.mark_snapshot_put_attempted(instance, snapshot.id, &source)
            .is_err()
    );
    repo.begin_stop(source.identity, &source.opaque_engine_key, 43)
        .unwrap();
    repo.finish_stopped(
        source.identity,
        &termination(&source.opaque_engine_key, Some(&source.start_identity)),
        44,
    )
    .unwrap();
    let root = fixture.directory.path().join("data");
    drop(fixture.storage);
    let storage = PlatformStorage::bootstrap(&configuration(&root), &SystemClock).unwrap();
    let repo = ContainerAuthority::new(storage.db());
    let recovered = repo.snapshots(instance).unwrap();
    assert_eq!(recovered.len(), 1);
    assert!(recovered[0].put_attempted);
    assert_eq!(recovered[0].state, ContainerSnapshotState::Retiring);
    assert_eq!(recovered[0].captured_at_ms, None);
    // A HEAD/DELETE absence result cannot exclude a delayed PUT/retry after that observation.
    assert!(
        repo.finish_snapshot_retirement(instance, snapshot.id, true)
            .is_err()
    );
    assert!(
        repo.finish_snapshot_retirement(instance, snapshot.id, false)
            .is_err()
    );
    assert!(
        repo.finish_snapshot_capture(&fixture.authority, snapshot.id, &source, 1502, 45)
            .is_err()
    );
    let key = "e".repeat(64);
    let replacement = repo
        .reserve(
            &fixture.authority,
            &key,
            &execution(&storage, &fixture.authority, &fixture.spec, &key),
            10,
            46,
        )
        .unwrap();
    let start = "f".repeat(64);
    repo.mark_starting(replacement.identity, &key, 47).unwrap();
    repo.observe_running(
        replacement.identity,
        &observe(&key, &start, &fixture.spec),
        48,
    )
    .unwrap();
    let request = NewContainerSnapshot {
        directory: None,
        id: ResourceId::generate(),
        source: ContainerSnapshotSource {
            identity: replacement.identity,
            opaque_engine_key: key,
            start_identity: start,
        },
        name: None,
        reserved_bytes: 1,
    };
    assert_eq!(
        repo.reserve_snapshot(
            &fixture.authority,
            &request,
            ContainerSnapshotLimits {
                max_snapshots: 4,
                max_bytes: 4096
            },
            49
        )
        .unwrap_err()
        .code(),
        ErrorCode::AdmissionBusy
    );
    assert_eq!(repo.snapshots(instance).unwrap(), recovered);
}

#[test]
fn positive_publication_retires_normally_with_or_without_an_external_put() {
    for attempted in [false, true] {
        let fixture = fixture(4);
        let repo = ContainerAuthority::new(fixture.storage.db());
        let instance = fixture.authority.instance_id;
        let source = source(&fixture);
        let snapshot = pending(&fixture, &source, 40);
        repo.stage_snapshot_artifact(instance, snapshot.id, &source, &image(21))
            .unwrap();
        if attempted {
            repo.mark_snapshot_put_attempted(instance, snapshot.id, &source)
                .unwrap();
        }
        // An already-existing verified object has no new PUT attempt to record.
        let published = repo
            .finish_snapshot_capture(&fixture.authority, snapshot.id, &source, 1502, 41)
            .unwrap();
        assert_eq!(published.put_attempted, attempted);
        assert_eq!(published.captured_at_ms, Some(41));
        assert!(
            repo.mark_snapshot_put_attempted(instance, snapshot.id, &source)
                .is_err()
        );
        repo.retire_snapshots(instance, 41 + CONTAINER_SNAPSHOT_RETENTION_MS)
            .unwrap();
        repo.finish_snapshot_retirement(instance, snapshot.id, true)
            .unwrap();
        assert!(repo.snapshots(instance).unwrap().is_empty());
    }
}

#[test]
fn default_scheduling_cannot_capture_or_restore_snapshots() {
    let fixture = super::fixture(2);
    let repo = ContainerAuthority::new(fixture.storage.db());
    let source = source(&fixture);
    assert!(
        repo.reserve_snapshot(
            &fixture.authority,
            &NewContainerSnapshot {
                directory: None,
                id: ResourceId::generate(),
                source: source.clone(),
                name: None,
                reserved_bytes: 4096
            },
            limits(),
            40
        )
        .is_err()
    );
    assert!(
        repo.snapshots(fixture.authority.instance_id)
            .unwrap()
            .is_empty()
    );
    assert!(
        repo.snapshot_for_restore(&fixture.authority, ResourceId::generate(), 41)
            .is_err()
    );
    assert_eq!(
        repo.lease(fixture.authority.instance_id, source.identity)
            .unwrap()
            .state,
        ContainerLeaseState::Running
    );
}

#[test]
fn snapshot_image_size_survives_restart_and_restore_without_changing_artifact_charge() {
    for image_size in [0, 1502, i64::MAX as u64] {
        let fixture = fixture(4);
        let instance = fixture.authority.instance_id;
        let repo = ContainerAuthority::new(fixture.storage.db());
        let source = source(&fixture);
        let snapshot = pending(&fixture, &source, 40);
        assert_eq!(snapshot.image_size, None);
        repo.stage_snapshot_artifact(instance, snapshot.id, &source, &image(21))
            .unwrap();
        assert_eq!(repo.snapshots(instance).unwrap()[0].image_size, None);
        let published = repo
            .finish_snapshot_capture(&fixture.authority, snapshot.id, &source, image_size, 41)
            .unwrap();
        assert_eq!(published.image_size, Some(image_size));
        assert_eq!(published.artifact_size, Some(2048));
        assert_eq!(published.artifact_sha256, Some(image(21).artifact_sha256));
        let other = other_object(&fixture, 19);
        let data = fixture.directory.path().join("data");
        drop(fixture.storage);
        let storage = PlatformStorage::bootstrap(&configuration(&data), &SystemClock).unwrap();
        let repo = ContainerAuthority::new(storage.db());
        assert_eq!(repo.snapshots(instance).unwrap(), vec![published.clone()]);
        let key = "c".repeat(64);
        let mut spec = execution(&storage, &other, &fixture.spec, &key);
        spec.image_sha256 = image(21).manifest_sha256;
        spec.startup.snapshot_id = Some(snapshot.id);
        spec.startup.image_reference.clear();
        let lease = repo.reserve(&other, &key, &spec, 10, 42).unwrap();
        repo.mark_starting(lease.identity, &key, 43).unwrap();
        let start = "d".repeat(64);
        let mut observed = observe(&key, &start, &fixture.spec);
        observed.image_sha256 = &spec.image_sha256;
        repo.observe_running(lease.identity, &observed, 44).unwrap();
        let restored = &repo.snapshots(instance).unwrap()[0];
        assert_eq!(restored.image_size, Some(image_size));
        assert_eq!(restored.last_restored_at_ms, Some(44));
        assert_eq!(restored.artifact_size, Some(2048));
        assert!(
            WorkerRepository::new(storage.db())
                .referenced_artifacts()
                .unwrap()
                .contains(&(image(21).artifact_sha256, 2048))
        );
        assert_eq!(
            repo.reserve_snapshot(
                &fixture.authority,
                &NewContainerSnapshot {
                    directory: None,
                    id: ResourceId::generate(),
                    source: source.clone(),
                    name: None,
                    reserved_bytes: 1,
                },
                ContainerSnapshotLimits {
                    max_snapshots: 4,
                    max_bytes: 2048,
                },
                45,
            )
            .unwrap_err()
            .code(),
            ErrorCode::AdmissionBusy
        );
    }
}

#[test]
fn snapshot_image_size_completion_rejects_stale_authority_overflow_and_tampering() {
    let fixture = fixture(4);
    let instance = fixture.authority.instance_id;
    let repo = ContainerAuthority::new(fixture.storage.db());
    let source = source(&fixture);
    let snapshot = pending(&fixture, &source, 40);
    repo.stage_snapshot_artifact(instance, snapshot.id, &source, &image(21))
        .unwrap();
    for image_size in [i64::MAX as u64 + 1, u64::MAX] {
        assert!(
            repo.finish_snapshot_capture(&fixture.authority, snapshot.id, &source, image_size, 41)
                .is_err()
        );
    }
    let mut stale = fixture.authority;
    stale.route_generation += 1;
    assert!(
        repo.finish_snapshot_capture(&stale, snapshot.id, &source, 1502, 41)
            .is_err()
    );
    let mut wrong = source.clone();
    wrong.start_identity = "c".repeat(64);
    assert!(
        repo.finish_snapshot_capture(&fixture.authority, snapshot.id, &wrong, 1502, 41)
            .is_err()
    );
    assert_eq!(repo.snapshots(instance).unwrap()[0].image_size, None);
    assert_eq!(repo.snapshots(instance).unwrap()[0].captured_at_ms, None);
    fixture.storage.db().with_immediate(|tx| {
        for sql in [
            "UPDATE container_snapshots SET image_size=1502 WHERE id=?1",
            "UPDATE container_snapshots SET state='ready',captured_at_ms=41 WHERE id=?1",
            "UPDATE container_snapshots SET state='ready',captured_at_ms=41,image_size=-1 WHERE id=?1",
            "UPDATE container_snapshots SET state='ready',captured_at_ms=41,image_size=9223372036854775808 WHERE id=?1",
        ] {
            assert!(tx.execute(sql, [snapshot.id]).is_err());
        }
        Ok(())
    }).unwrap();
    let published = repo
        .finish_snapshot_capture(&fixture.authority, snapshot.id, &source, 0, 41)
        .unwrap();
    assert!(
        repo.finish_snapshot_capture(&fixture.authority, snapshot.id, &source, 2850, 42)
            .is_err()
    );
    fixture
        .storage
        .db()
        .with_immediate(|tx| {
            for sql in [
                "UPDATE container_snapshots SET image_size=2850 WHERE id=?1",
                "UPDATE container_snapshots SET image_size=NULL WHERE id=?1",
                "UPDATE container_snapshots SET image_size=-1 WHERE id=?1",
                "UPDATE container_snapshots SET state='retiring',image_size=2850 WHERE id=?1",
            ] {
                assert!(tx.execute(sql, [snapshot.id]).is_err());
            }
            Ok(())
        })
        .unwrap();
    assert_eq!(repo.snapshots(instance).unwrap(), vec![published]);
    assert_eq!(
        repo.snapshot_for_restore(&other_object(&fixture, 19), snapshot.id, 42)
            .unwrap(),
        image(21)
    );
}

#[test]
fn snapshot_ready_without_image_size_fails_closed_on_read_and_restart() {
    let fixture = fixture(4);
    let instance = fixture.authority.instance_id;
    let repo = ContainerAuthority::new(fixture.storage.db());
    let source = source(&fixture);
    let snapshot = pending(&fixture, &source, 40);
    repo.stage_snapshot_artifact(instance, snapshot.id, &source, &image(21))
        .unwrap();
    fixture
        .storage
        .db()
        .with_immediate(|tx| {
            // Simulate corrupt persisted state without changing the declared schema.
            tx.pragma_update(None, "ignore_check_constraints", "ON")
                .unwrap();
            tx.execute(
                "UPDATE container_snapshots SET state='ready',captured_at_ms=41 WHERE id=?1",
                [snapshot.id],
            )
            .unwrap();
            tx.pragma_update(None, "ignore_check_constraints", "OFF")
                .unwrap();
            Ok(())
        })
        .unwrap();
    assert!(repo.snapshots(instance).is_err());
    assert!(
        repo.snapshot_for_restore(&fixture.authority, snapshot.id, 42)
            .is_err()
    );
    assert!(crate::migrations::apply(fixture.storage.db(), &SystemClock).is_err());
    fixture
        .storage
        .db()
        .with_read(|conn| {
            assert_eq!(
                conn.query_row(
                    "SELECT image_size FROM container_snapshots WHERE id=?1",
                    [snapshot.id],
                    |row| row.get::<_, Option<i64>>(0)
                )
                .unwrap(),
                None
            );
            Ok(())
        })
        .unwrap();
    let data = fixture.directory.path().join("data");
    drop(fixture.storage);
    assert!(PlatformStorage::bootstrap(&configuration(&data), &SystemClock).is_err());
}

#[path = "containers_directory_restore_tests.rs"]
mod directory_restore_tests;
