use super::*;

#[test]
fn container_rollout_creation_matches_live_target_atomically_and_rejects_deletion() {
    let fixture = fixture(2);
    let instance = fixture.storage.identity().instance_id;
    let repo = ContainerAuthority::new(fixture.storage.db());
    let app = repo
        .application_for_namespace(
            instance,
            fixture.authority.namespace_resource_id,
            "container-app",
        )
        .unwrap();
    let original = br#"{"image":"original","vcpu":1.0}"#;
    repo.commit_application_configuration(
        instance,
        &app,
        &fixture.spec,
        original,
        ContainerApplicationMetadataPatch::default(),
        30,
    )
    .unwrap();
    repo.admit_image(instance, &image(2)).unwrap();
    let mut replacement = fixture.spec.clone();
    replacement.image_sha256 = Some([2; 32]);
    let configuration = br#"{"image":"replacement","vcpu":1.0}"#;
    let target = repo
        .commit_application_configuration(
            instance,
            &app,
            &replacement,
            configuration,
            ContainerApplicationMetadataPatch::default(),
            31,
        )
        .unwrap();
    let stale = ContainerRolloutCreate {
        id: ResourceId::generate(),
        target_configuration: original,
        steps: &[100],
        details: None,
    };
    assert_eq!(
        repo.create_rollout(instance, app.id, &stale, 32)
            .unwrap_err()
            .code(),
        ErrorCode::VersionNotReady,
    );
    assert!(repo.rollouts(instance, app.id).unwrap().is_empty());
    let current = ContainerRolloutCreate {
        id: ResourceId::generate(),
        target_configuration: br#"{"vcpu":1.0,"image":"replacement"}"#,
        steps: &[100],
        details: None,
    };
    let rollout = repo.create_rollout(instance, app.id, &current, 33).unwrap();
    assert_eq!(rollout.target_generation, target.generation);
    assert_eq!(
        repo.target_configuration(instance, app.id, target.generation)
            .unwrap()
            .unwrap(),
        configuration,
    );
    repo.rollout_step_leases(instance, app.id, rollout.id, 34)
        .unwrap();
    let missing = ResourceId::generate();
    assert_eq!(
        repo.create_rollout(
            instance,
            missing,
            &ContainerRolloutCreate {
                id: ResourceId::generate(),
                ..current
            },
            35
        )
        .unwrap_err()
        .code(),
        ErrorCode::ResourceNotFound,
    );
    repo.begin_application_delete(instance, app.id, 36).unwrap();
    let pending = ContainerRolloutCreate {
        id: ResourceId::generate(),
        ..current
    };
    assert_eq!(
        repo.create_rollout(instance, app.id, &pending, 37)
            .unwrap_err()
            .code(),
        ErrorCode::ResourceNotFound,
    );
    assert_eq!(repo.rollouts(instance, app.id).unwrap().len(), 1);
    repo.finish_application_delete(instance, app.id, 38)
        .unwrap();
    let deleted = ContainerRolloutCreate {
        id: ResourceId::generate(),
        ..current
    };
    assert_eq!(
        repo.create_rollout(instance, app.id, &deleted, 39)
            .unwrap_err()
            .code(),
        ErrorCode::ResourceNotFound,
    );
    assert_eq!(
        repo.rollout(instance, app.id, rollout.id)
            .unwrap()
            .target_generation,
        target.generation
    );
}

#[test]
fn container_target_configuration_is_immutable_identity_and_code_only_reuses_it() {
    let fixture = fixture(2);
    let instance = fixture.storage.identity().instance_id;
    let repo = ContainerAuthority::new(fixture.storage.db());
    let app = repo
        .application_for_namespace(
            instance,
            fixture.authority.namespace_resource_id,
            "container-app",
        )
        .unwrap();
    let digest = hex::encode(fixture.spec.image_sha256.unwrap());
    let first = serde_json::to_vec(&serde_json::json!({"image":format!("registry.example.test/repo-a@sha256:{digest}"),"vcpu":0.0625,"memory_mib":256,"disk":{"size_mb":1073.741824}})).unwrap();
    let original = repo
        .commit_application_configuration(
            instance,
            &app,
            &fixture.spec,
            &first,
            ContainerApplicationMetadataPatch::default(),
            30,
        )
        .unwrap();
    let second = serde_json::to_vec(&serde_json::json!({"image":format!("registry.example.test/repo-b@sha256:{digest}"),"vcpu":0.0625,"memory_mib":256,"disk":{"size_mb":1073.741824}})).unwrap();
    let current = repo
        .commit_application_configuration(
            instance,
            &app,
            &fixture.spec,
            &second,
            ContainerApplicationMetadataPatch::default(),
            31,
        )
        .unwrap();
    assert_eq!(current.generation, original.generation + 1);
    assert_eq!(current.spec, original.spec);
    assert_eq!(
        repo.target_configuration(instance, app.id, original.generation)
            .unwrap()
            .unwrap(),
        first
    );
    assert_eq!(
        repo.application_settings(instance, app.id).unwrap().0,
        Some(second.clone())
    );
    assert_eq!(
        repo.target_configuration(instance, app.id, current.generation)
            .unwrap()
            .unwrap(),
        second
    );
    let request = ContainerRolloutCreate {
        id: ResourceId::generate(),
        target_configuration: &first,
        steps: &[100],
        details: None,
    };
    assert_eq!(
        repo.create_rollout(instance, app.id, &request, 32)
            .unwrap_err()
            .code(),
        ErrorCode::VersionNotReady
    );
    let request = ContainerRolloutCreate {
        id: ResourceId::generate(),
        target_configuration: &second,
        ..request
    };
    assert_eq!(
        repo.create_rollout(instance, app.id, &request, 33)
            .unwrap()
            .target_generation,
        current.generation
    );
    repo.rollout_step_leases(instance, app.id, request.id, 34)
        .unwrap();
    let next_version = version(&fixture.storage, fixture.worker);
    repo.declare_version(
        instance,
        next_version,
        app.namespace_resource_id,
        &app.name,
        &declaration_digest(app.namespace_resource_id, &app.name),
    )
    .unwrap();
    activate(&fixture.storage, fixture.worker, next_version);
    let mut code_only = fixture.spec.clone();
    code_only.worker_version_id = next_version;
    let same = repo
        .commit_application_configuration(
            instance,
            &app,
            &code_only,
            &second,
            ContainerApplicationMetadataPatch::default(),
            35,
        )
        .unwrap();
    assert_eq!(same.generation, current.generation);
    assert_eq!(same.spec.worker_version_id, current.spec.worker_version_id);
    assert_eq!(
        repo.target_configuration(instance, app.id, same.generation)
            .unwrap()
            .unwrap(),
        second
    );
    // Stored invalid JSON is corruption, not authority to overwrite during publication or rollout.
    fixture.storage.db().with_read(|conn| {
        conn.execute_batch("DROP TRIGGER container_target_configuration_immutable").unwrap();
        conn.execute("UPDATE container_target_configuration SET configuration_json=?2 WHERE application_id=?1 AND target_generation=?3",
            params![app.id,b"{".as_slice(),current.generation]).unwrap(); Ok(())
    }).unwrap();
    assert_eq!(
        repo.create_rollout(
            instance,
            app.id,
            &ContainerRolloutCreate {
                id: ResourceId::generate(),
                ..request
            },
            36
        )
        .unwrap_err()
        .code(),
        ErrorCode::ResourceInvariantViolation
    );
    assert_eq!(
        repo.commit_application_configuration(
            instance,
            &app,
            &code_only,
            &second,
            ContainerApplicationMetadataPatch::default(),
            37
        )
        .unwrap_err()
        .code(),
        ErrorCode::ResourceInvariantViolation
    );
    assert_eq!(
        repo.target_configuration(instance, app.id, current.generation)
            .unwrap()
            .unwrap(),
        b"{"
    );
    assert_eq!(repo.rollouts(instance, app.id).unwrap().len(), 1);
}

#[test]
fn container_active_rollout_fences_new_targets_but_allows_same_target_and_metadata() {
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
    repo.admit_image(instance, &image(2)).unwrap();
    let mut target_spec = fixture.spec.clone();
    target_spec.image_sha256 = Some([2; 32]);
    let configuration = br#"{"image":"replacement"}"#;
    let target = repo
        .commit_application_configuration(
            instance,
            &app,
            &target_spec,
            configuration,
            ContainerApplicationMetadataPatch::default(),
            33,
        )
        .unwrap();
    let rollout = repo
        .create_rollout(
            instance,
            app.id,
            &ContainerRolloutCreate {
                id: ResourceId::generate(),
                target_configuration: configuration,
                steps: &[100],
                details: None,
            },
            34,
        )
        .unwrap();
    let frozen_configuration = configuration.to_vec();
    let settings = repo.application_settings(instance, app.id).unwrap();
    let before = repo
        .application_for_namespace(instance, app.namespace_resource_id, &app.name)
        .unwrap();
    let leases = repo.application_leases(instance, app.id).unwrap();
    assert_eq!(
        repo.rollout_progress(instance, app.id, rollout.id).unwrap(),
        (1, 0)
    );
    repo.admit_image(instance, &image(3)).unwrap();
    let mut next_spec = target_spec.clone();
    next_spec.image_sha256 = Some([3; 32]);
    for (spec, configuration) in [
        (&next_spec, br#"{"image":"next"}"#.as_slice()),
        (
            &target_spec,
            br#"{"image":"same-digest-new-reference"}"#.as_slice(),
        ),
    ] {
        assert_eq!(
            repo.commit_application_configuration(
                instance,
                &app,
                spec,
                configuration,
                ContainerApplicationMetadataPatch {
                    max_instances: Some(2),
                    observability_logs: Some(true)
                },
                35
            )
            .unwrap_err()
            .code(),
            ErrorCode::VersionNotReady
        );
        assert_eq!(
            repo.application_for_namespace(instance, app.namespace_resource_id, &app.name)
                .unwrap(),
            before
        );
        assert_eq!(
            repo.application_settings(instance, app.id).unwrap(),
            settings
        );
        assert_eq!(repo.application_leases(instance, app.id).unwrap(), leases);
        assert_eq!(repo.rollout(instance, app.id, rollout.id).unwrap(), rollout);
        assert_eq!(
            repo.rollout_progress(instance, app.id, rollout.id).unwrap(),
            (1, 0)
        );
        assert!(
            repo.target(instance, app.id, target.generation + 1)
                .is_err()
        );
        assert_eq!(
            repo.target_configuration(instance, app.id, target.generation)
                .unwrap()
                .unwrap(),
            frozen_configuration
        );
    }
    let version = version(&fixture.storage, fixture.worker);
    repo.declare_version(
        instance,
        version,
        app.namespace_resource_id,
        &app.name,
        &declaration_digest(app.namespace_resource_id, &app.name),
    )
    .unwrap();
    activate(&fixture.storage, fixture.worker, version);
    let mut code_only = target_spec.clone();
    code_only.worker_version_id = version;
    let same = repo
        .commit_application_configuration(
            instance,
            &app,
            &code_only,
            configuration,
            ContainerApplicationMetadataPatch::default(),
            36,
        )
        .unwrap();
    assert_eq!(same, target);
    repo.patch_application_metadata(
        instance,
        app.id,
        ContainerApplicationMetadataPatch {
            max_instances: Some(2),
            observability_logs: Some(true),
        },
        37,
    )
    .unwrap();
    let after = repo
        .application_for_namespace(instance, app.namespace_resource_id, &app.name)
        .unwrap();
    assert_eq!(after.target_generation, Some(target.generation));
    assert_eq!(after.max_instances, 2);
    assert_eq!(
        repo.application_settings(instance, app.id).unwrap(),
        (Some(configuration.to_vec()), true, 37)
    );
    assert_eq!(repo.rollout(instance, app.id, rollout.id).unwrap(), rollout);
    assert_eq!(
        repo.rollout_progress(instance, app.id, rollout.id).unwrap(),
        (1, 0)
    );
    assert_eq!(repo.application_leases(instance, app.id).unwrap(), leases);
}

#[test]
fn container_duplicate_active_rollout_is_conflict_and_completed_rollout_allows_next() {
    let fixture = fixture(2);
    let instance = fixture.storage.identity().instance_id;
    let repo = ContainerAuthority::new(fixture.storage.db());
    let app = repo
        .application_for_namespace(
            instance,
            fixture.authority.namespace_resource_id,
            "container-app",
        )
        .unwrap();
    let configuration = repo
        .target_configuration(instance, app.id, app.target_generation.unwrap())
        .unwrap()
        .unwrap();
    let request = ContainerRolloutCreate {
        id: ResourceId::generate(),
        target_configuration: &configuration,
        steps: &[100],
        details: None,
    };
    let first = repo.create_rollout(instance, app.id, &request, 30).unwrap();
    let settings = repo.application_settings(instance, app.id).unwrap();
    let target = repo
        .target(instance, app.id, first.target_generation)
        .unwrap();
    let leases = repo.application_leases(instance, app.id).unwrap();
    let duplicate = ContainerRolloutCreate {
        id: ResourceId::generate(),
        ..request
    };
    let error = repo
        .create_rollout(instance, app.id, &duplicate, 31)
        .unwrap_err();
    assert_eq!(error.code(), ErrorCode::VersionNotReady);
    assert_eq!(
        repo.rollouts(instance, app.id).unwrap(),
        vec![first.clone()]
    );
    assert!(repo.rollout(instance, app.id, duplicate.id).is_err());
    assert_eq!(
        repo.application_for_namespace(instance, app.namespace_resource_id, &app.name)
            .unwrap(),
        app
    );
    assert_eq!(
        repo.application_settings(instance, app.id).unwrap(),
        settings
    );
    assert_eq!(
        repo.target(instance, app.id, first.target_generation)
            .unwrap(),
        target
    );
    assert_eq!(repo.application_leases(instance, app.id).unwrap(), leases);
    assert_eq!(
        repo.rollout_progress(instance, app.id, first.id).unwrap(),
        (0, 0)
    );
    assert!(
        repo.rollout_step_leases(instance, app.id, first.id, 32)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        repo.rollout(instance, app.id, first.id).unwrap().state,
        "completed"
    );
    let next = repo
        .create_rollout(instance, app.id, &duplicate, 33)
        .unwrap();
    assert_eq!(next.id, duplicate.id);
    assert_eq!(next.state, "running");
    assert_eq!(next.target_generation, first.target_generation);
    assert_eq!(repo.rollouts(instance, app.id).unwrap().len(), 2);
}
