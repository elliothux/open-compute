use super::*;

fn insert(
    storage: &PlatformStorage,
    version: VersionId,
    declaration: &VersionContainerDeclaration,
) -> Result<(), PlatformError> {
    storage.db().with_immediate(|tx| {
        insert_version_declarations(tx, version, std::slice::from_ref(declaration), None)
    })
}

fn remove_application(fixture: &Fixture, application: ResourceId) {
    fixture
        .storage
        .db()
        .with_immediate(|tx| {
            tx.execute(
                "DELETE FROM container_application_settings WHERE application_id=?1",
                [application],
            )
            .map_err(|_| invariant())?;
            tx.execute(
                "DELETE FROM container_target_configuration WHERE application_id=?1",
                [application],
            )
            .map_err(|_| invariant())?;
            tx.execute(
                "DELETE FROM container_application_targets WHERE application_id=?1",
                [application],
            )
            .map_err(|_| invariant())?;
            tx.execute(
                "DELETE FROM container_applications WHERE id=?1",
                [application],
            )
            .map_err(|_| invariant())?;
            Ok(())
        })
        .unwrap();
}

#[test]
fn named_container_images_are_immutable_admitted_version_authority_across_reopen() {
    let fixture = fixture_with_policy(4, true);
    let instance = fixture.authority.instance_id;
    let namespace = fixture.authority.namespace_resource_id;
    let repo = ContainerAuthority::new(fixture.storage.db());
    repo.admit_image(instance, &image(2)).unwrap();
    let references = BTreeMap::from([
        (
            "blue".into(),
            format!(
                "registry/app@sha256:{}",
                hex::encode(image(1).manifest_sha256)
            ),
        ),
        (
            "绿色".into(),
            format!(
                "registry/app@sha256:{}",
                hex::encode(image(2).manifest_sha256)
            ),
        ),
    ]);
    let declaration = VersionContainerDeclaration::with_images(
        namespace,
        "container-app".into(),
        references.clone(),
    )
    .unwrap();
    let first = version(&fixture.storage, fixture.worker);
    insert(&fixture.storage, first, &declaration).unwrap();
    assert_eq!(
        repo.version_declarations(instance, first).unwrap(),
        vec![declaration.clone()]
    );
    assert!(fixture.storage.db().with_immediate(|tx| {
        tx.execute("UPDATE version_container_images SET image_reference='changed' WHERE version_id=?1",[first]).map_err(|_|invariant())?;
        Ok(())
    }).is_err());
    let route = activate(&fixture.storage, fixture.worker, first);
    assert!(
        repo.resolve_declared_application(
            instance,
            &declaration.identity(),
            fixture.authority.object_id,
            fixture.authority.object_generation,
            route
        )
        .unwrap()
        .is_some()
    );
    let empty_identity = VersionContainerDeclaration::new(namespace, "container-app".into())
        .unwrap()
        .identity();
    assert!(
        repo.resolve_declared_application(
            instance,
            &empty_identity,
            fixture.authority.object_id,
            fixture.authority.object_generation,
            route
        )
        .is_err()
    );
    let successor = version(&fixture.storage, fixture.worker);
    let replacement = VersionContainerDeclaration::with_images(
        namespace,
        "container-app".into(),
        BTreeMap::from([("blue".into(), references["绿色"].clone())]),
    )
    .unwrap();
    insert(&fixture.storage, successor, &replacement).unwrap();
    assert_ne!(declaration.descriptor_sha256, replacement.descriptor_sha256);
    let next_route = activate(&fixture.storage, fixture.worker, successor);
    assert!(next_route > route);
    assert!(
        repo.resolve_declared_application(
            instance,
            &declaration.identity(),
            fixture.authority.object_id,
            fixture.authority.object_generation,
            next_route
        )
        .is_err()
    );
    assert!(
        repo.resolve_declared_application(
            instance,
            &replacement.identity(),
            fixture.authority.object_id,
            fixture.authority.object_generation,
            next_route
        )
        .unwrap()
        .is_some()
    );
    assert_eq!(
        repo.version_declarations(instance, first).unwrap()[0].images,
        references
    );
    assert_eq!(
        repo.version_declarations(instance, successor).unwrap(),
        vec![replacement.clone()]
    );
    let retained = WorkerRepository::new(fixture.storage.db())
        .referenced_artifacts()
        .unwrap();
    for fill in [1, 2] {
        let image = image(fill);
        assert!(retained.contains(&(image.artifact_sha256, image.artifact_size)));
    }
    let missing = VersionContainerDeclaration::with_images(
        namespace,
        "container-app".into(),
        BTreeMap::from([(
            "unknown".into(),
            format!("registry/app@sha256:{}", "9".repeat(64)),
        )]),
    )
    .unwrap();
    let rejected = version(&fixture.storage, fixture.worker);
    assert!(insert(&fixture.storage, rejected, &missing).is_err());
    assert!(
        repo.version_declarations(instance, rejected)
            .unwrap()
            .is_empty()
    );
    assert!(
        VersionContainerDeclaration::with_images(
            namespace,
            "container-app".into(),
            BTreeMap::from([("mutable".into(), "registry/app:latest".into()),])
        )
        .is_err()
    );
    assert!(
        VersionContainerDeclaration::with_images(
            namespace,
            "container-app".into(),
            BTreeMap::from([("x".repeat(16384), references["blue"].clone()),])
        )
        .is_err()
    );
    let removed = staging_authority::stage(
        &fixture.storage,
        fixture.worker,
        &VersionContainerDeclaration::new(namespace, "container-app".into()).unwrap(),
        None,
        40,
    )
    .unwrap();
    let removed_route = activate(&fixture.storage, fixture.worker, removed);
    assert!(removed_route > next_route);
    assert!(
        repo.version_declarations(instance, removed).unwrap()[0]
            .images
            .is_empty()
    );
    assert!(
        repo.resolve_declared_application(
            instance,
            &empty_identity,
            fixture.authority.object_id,
            fixture.authority.object_generation,
            removed_route,
        )
        .unwrap()
        .is_some()
    );
    let root = fixture.directory.path().join("data");
    drop(fixture.storage);
    let storage = PlatformStorage::bootstrap(&configuration(&root), &SystemClock).unwrap();
    let repo = ContainerAuthority::new(storage.db());
    assert_eq!(
        repo.version_declarations(instance, first).unwrap(),
        vec![declaration]
    );
    assert_eq!(
        repo.version_declarations(instance, successor).unwrap(),
        vec![replacement]
    );
    assert!(
        repo.version_declarations(instance, removed).unwrap()[0]
            .images
            .is_empty()
    );
    // Fault injection proves reads reject mismatched persisted reference/hash authority.
    storage
        .db()
        .with_immediate(|tx| {
            tx.execute_batch("DROP TRIGGER version_container_image_update_guard")
                .map_err(|_| invariant())?;
            tx.execute(
                "UPDATE version_container_images SET image_reference=?2 WHERE version_id=?1",
                params![successor, references["blue"]],
            )
            .map_err(|_| invariant())?;
            Ok(())
        })
        .unwrap();
    assert!(repo.version_declarations(instance, successor).is_err());
}

#[test]
fn default_application_rejects_named_images_in_initial_configuration_and_redeploy_atomically() {
    let fixture = fixture(2);
    let instance = fixture.authority.instance_id;
    let namespace = fixture.authority.namespace_resource_id;
    let repo = ContainerAuthority::new(fixture.storage.db());
    let declaration = VersionContainerDeclaration::with_images(
        namespace,
        "container-app".into(),
        BTreeMap::from([(
            "blue".into(),
            format!(
                "registry/app@sha256:{}",
                hex::encode(image(1).manifest_sha256)
            ),
        )]),
    )
    .unwrap();
    let before = fixture
        .storage
        .db()
        .with_read(|conn| {
            conn.query_row("SELECT COUNT(*) FROM worker_versions", [], |row| {
                row.get::<_, u64>(0)
            })
            .map_err(|_| invariant())
        })
        .unwrap();
    let error = staging_authority::stage(&fixture.storage, fixture.worker, &declaration, None, 40)
        .unwrap_err();
    assert_eq!(error.code(), ErrorCode::ConfigInvalid);
    fixture
        .storage
        .db()
        .with_read(|conn| {
            assert_eq!(
                conn.query_row("SELECT COUNT(*) FROM worker_versions", [], |row| row
                    .get::<_, u64>(0))
                    .unwrap(),
                before
            );
            assert_eq!(
                conn.query_row("SELECT COUNT(*) FROM version_container_images", [], |row| {
                    row.get::<_, u64>(0)
                })
                .unwrap(),
                0
            );
            Ok(())
        })
        .unwrap();
    assert_eq!(
        repo.application_for_namespace(instance, namespace, "container-app")
            .unwrap()
            .target_generation,
        Some(1)
    );
    assert_eq!(
        repo.application_settings(instance, fixture.authority.application_id)
            .unwrap()
            .0,
        Some(b"{}".to_vec())
    );
    // Initial upload may precede application creation. Its immutable map cannot become a default target.
    remove_application(&fixture, fixture.authority.application_id);
    let uploaded =
        staging_authority::stage(&fixture.storage, fixture.worker, &declaration, None, 41).unwrap();
    let workers = WorkerRepository::new(fixture.storage.db());
    workers.begin_validation(uploaded).unwrap();
    let app = ContainerApplication {
        id: ResourceId::generate(),
        namespace_resource_id: namespace,
        name: "container-app".into(),
        scheduling_policy: ContainerSchedulingPolicy::Default,
        max_instances: 2,
        target_generation: None,
        admission_paused: true,
        created_at_ms: 42,
    };
    // The map was staged before the default application existed. Ready and activation
    // independently recheck the association inside their own write transactions.
    let metadata = ContainerApplicationMetadataPatch {
        max_instances: None,
        observability_logs: Some(false),
    };
    repo.commit_application_configuration(instance, &app, &fixture.spec, b"{}", metadata, 42)
        .unwrap();
    assert_eq!(
        workers.mark_ready(uploaded, 43).unwrap_err().code(),
        ErrorCode::ConfigInvalid
    );
    remove_application(&fixture, app.id);
    workers.mark_ready(uploaded, 44).unwrap();
    repo.commit_application_configuration(instance, &app, &fixture.spec, b"{}", metadata, 45)
        .unwrap();
    let before = workers.get_worker(instance, fixture.worker).unwrap();
    assert_eq!(
        workers
            .promote(
                instance,
                fixture.worker,
                uploaded,
                None,
                RequestId::generate(),
                46
            )
            .unwrap_err()
            .code(),
        ErrorCode::ConfigInvalid
    );
    let after = workers.get_worker(instance, fixture.worker).unwrap();
    assert_eq!(after.active_version_id, before.active_version_id);
    assert_eq!(after.route_generation, before.route_generation);
    assert_eq!(
        repo.application_settings(instance, app.id).unwrap().0,
        Some(b"{}".to_vec())
    );
    remove_application(&fixture, app.id);
    workers
        .promote(
            instance,
            fixture.worker,
            uploaded,
            None,
            RequestId::generate(),
            47,
        )
        .unwrap();
    // A stale empty-map create cannot race a newly active named-image Version.
    assert_eq!(
        repo.commit_application_configuration(instance, &app, &fixture.spec, b"{}", metadata, 48)
            .unwrap_err()
            .code(),
        ErrorCode::ConfigInvalid
    );
    assert!(repo.applications(instance).unwrap().is_empty());
    let error = repo
        .commit_application_configuration(
            instance,
            &app,
            &ContainerTargetSpec {
                worker_version_id: uploaded,
                ..fixture.spec.clone()
            },
            b"{}",
            ContainerApplicationMetadataPatch {
                max_instances: None,
                observability_logs: Some(false),
            },
            42,
        )
        .unwrap_err();
    assert_eq!(error.code(), ErrorCode::ConfigInvalid);
    assert!(repo.applications(instance).unwrap().is_empty());
    assert_eq!(
        repo.version_declarations(instance, uploaded).unwrap(),
        vec![declaration]
    );
}

#[test]
fn named_image_map_limits_count_names_unicode_and_immutable_reference_bytes() {
    let namespace = ResourceId::generate();
    let reference = format!("app@sha256:{}", "1".repeat(64));
    let map: BTreeMap<_, _> = (0..100)
        .map(|index| (format!("image-{index}"), reference.clone()))
        .collect();
    assert!(
        VersionContainerDeclaration::with_images(namespace, "container-app".into(), map.clone())
            .is_ok()
    );
    let mut too_many = map;
    too_many.insert("extra".into(), reference.clone());
    assert!(
        VersionContainerDeclaration::with_images(namespace, "container-app".into(), too_many)
            .is_err()
    );
    for name in ["a".repeat(128), "🦀".repeat(128)] {
        assert!(
            VersionContainerDeclaration::with_images(
                namespace,
                "container-app".into(),
                BTreeMap::from([(name, reference.clone())])
            )
            .is_ok()
        );
    }
    for name in [String::new(), "a".repeat(129), "🦀".repeat(129)] {
        assert!(
            VersionContainerDeclaration::with_images(
                namespace,
                "container-app".into(),
                BTreeMap::from([(name, reference.clone())])
            )
            .is_err()
        );
    }
}

#[test]
fn runtime_quarantine_commits_but_does_not_reactivate_named_images_for_a_default_application() {
    let fixture = fixture(2);
    let instance = fixture.authority.instance_id;
    let namespace = fixture.authority.namespace_resource_id;
    let repo = ContainerAuthority::new(fixture.storage.db());
    let workers = WorkerRepository::new(fixture.storage.db());
    remove_application(&fixture, fixture.authority.application_id);
    let declaration = VersionContainerDeclaration::with_images(
        namespace,
        "container-app".into(),
        BTreeMap::from([(
            "blue".into(),
            format!(
                "registry/app@sha256:{}",
                hex::encode(image(1).manifest_sha256)
            ),
        )]),
    )
    .unwrap();
    let named =
        staging_authority::stage(&fixture.storage, fixture.worker, &declaration, None, 30).unwrap();
    workers.begin_validation(named).unwrap();
    workers.mark_ready(named, 31).unwrap();
    workers
        .promote(
            instance,
            fixture.worker,
            named,
            None,
            RequestId::generate(),
            32,
        )
        .unwrap();
    let empty = VersionContainerDeclaration::new(namespace, "container-app".into()).unwrap();
    let current =
        staging_authority::stage(&fixture.storage, fixture.worker, &empty, None, 33).unwrap();
    workers.begin_validation(current).unwrap();
    workers.mark_ready(current, 34).unwrap();
    let active = workers
        .promote(
            instance,
            fixture.worker,
            current,
            Some(named),
            RequestId::generate(),
            35,
        )
        .unwrap();
    let deployment = active.active_deployment_id.unwrap();
    let app = ContainerApplication {
        id: fixture.authority.application_id,
        namespace_resource_id: namespace,
        name: "container-app".into(),
        scheduling_policy: ContainerSchedulingPolicy::Default,
        max_instances: 2,
        target_generation: None,
        admission_paused: true,
        created_at_ms: 40,
    };
    let target = repo
        .commit_application_configuration(
            instance,
            &app,
            &ContainerTargetSpec {
                worker_version_id: current,
                ..fixture.spec.clone()
            },
            b"{}",
            ContainerApplicationMetadataPatch {
                max_instances: None,
                observability_logs: Some(false),
            },
            40,
        )
        .unwrap();
    assert!(
        workers
            .quarantine_active_deployment(
                deployment,
                "RUNTIME_UNEXPECTED_EXIT",
                RequestId::generate(),
                41
            )
            .unwrap()
    );
    let fenced = workers.get_worker(instance, fixture.worker).unwrap();
    assert!(fenced.active_deployment_id.is_none());
    assert!(fenced.active_version_id.is_none());
    assert_eq!(fenced.route_generation, active.route_generation + 1);
    assert_eq!(
        workers
            .deployment_runtime_assessments()
            .unwrap()
            .quarantined,
        1
    );
    assert_eq!(
        repo.target(instance, app.id, target.generation).unwrap(),
        target
    );
    assert_eq!(
        repo.application_settings(instance, app.id).unwrap().0,
        Some(b"{}".to_vec())
    );
    assert_eq!(
        repo.version_declarations(instance, named).unwrap(),
        vec![declaration]
    );
    assert!(
        !workers
            .quarantine_active_deployment(deployment, "STALE", RequestId::generate(), 42)
            .unwrap()
    );
}
