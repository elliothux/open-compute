//! Control-side capability and operation atomicity; scheduler proofs are test-owned here.

use super::*;

fn v2(storage: &PlatformStorage, deployment: DeploymentId) -> WorkflowDefinition {
    let repo = WorkflowRepository::new(storage.db());
    let account = storage.identity().default_account_id;
    let definition = repo.create_definition(account, "durable", 0).unwrap();
    let version = repo
        .stage_version(account, definition.id, deployment, "Flow", 2, 1)
        .unwrap();
    repo.finish_version(account, version.target.version_id, true, 2)
        .unwrap();
    repo.definition(account, definition.id).unwrap()
}

#[test]
fn capability_is_explicit_and_retained_history_still_pins_but_does_not_consume_active_quota() {
    let (_temp, storage, deployment) = setup();
    let repo = WorkflowRepository::new(storage.db());
    let account = storage.identity().default_account_id;
    let definition = v2(&storage, deployment);
    let limits = WorkflowsConfig {
        max_active_per_account: 1,
        ..Default::default()
    };
    for capability in [0, 1, 3] {
        assert_eq!(
            repo.reserve_instance(account, definition.id, None, capability, &limits, 3)
                .unwrap_err()
                .code(),
            ErrorCode::WorkflowCapabilityMismatch
        );
    }
    for capability in [0, 3] {
        assert_eq!(
            repo.stage_version(account, definition.id, deployment, "Flow", capability, 3)
                .unwrap_err()
                .code(),
            ErrorCode::WorkflowCapabilityMismatch
        );
    }
    let version = repo
        .version(account, definition.current_version_id.unwrap())
        .unwrap();
    let mut changed = version.target.clone();
    changed.capability_version = 1;
    assert_ne!(
        version_digest(&changed).unwrap(),
        version.target.descriptor_sha256
    );
    let binding = repo
        .prepare_binding(
            account,
            DeploymentId::generate(),
            "FLOW",
            definition.id,
            2,
            3,
        )
        .unwrap();
    assert_eq!(binding.descriptor.capability_version, 2);
    let mut changed = binding.descriptor.clone();
    changed.capability_version = 1;
    assert_ne!(changed.sha256().unwrap(), binding.descriptor_sha256);
    changed.capability_version = 3;
    assert!(changed.sha256().is_err());
    let first = repo
        .reserve_instance(account, definition.id, Some("first"), 2, &limits, 4)
        .unwrap()
        .identity;
    assert_eq!(
        repo.retain_instance(&first, 5).unwrap_err().code(),
        ErrorCode::WorkflowInstanceStateConflict
    );
    repo.finalize_instance(&first, 5).unwrap();
    assert_eq!(
        repo.release_instance(&first, 6).unwrap_err().code(),
        ErrorCode::WorkflowMethodUnsupported
    );
    repo.retain_instance(&first, 6).unwrap();
    repo.retain_instance(&first, 6).unwrap();
    assert!(repo.instance_referrers_intact(&first).unwrap());
    assert_eq!(
        repo.delete(account, definition.id, 7).unwrap_err().code(),
        ErrorCode::WorkflowReferenced
    );
    let second = repo
        .reserve_instance(account, definition.id, Some("second"), 2, &limits, 7)
        .unwrap()
        .identity;
    assert_eq!(
        repo.prepare_instance_operation(
            &first,
            open_compute_core::WorkflowOperationId::generate(),
            WorkflowOperationKind::Restart,
            &limits,
            8
        )
        .unwrap_err()
        .code(),
        ErrorCode::WorkflowStateQuotaExceeded
    );
    assert!(
        repo.instance_operation(first.instance_id)
            .unwrap()
            .is_none()
    );
    repo.abandon_creation(&second).unwrap();
    let operation = repo
        .prepare_instance_operation(
            &first,
            open_compute_core::WorkflowOperationId::generate(),
            WorkflowOperationKind::Restart,
            &limits,
            9,
        )
        .unwrap();
    assert_eq!(operation.target_generation(), 2);
    assert_eq!(operation.identity(), &first);
    assert_eq!(
        repo.instance_operations(None, 1).unwrap(),
        vec![operation.clone()]
    );
    assert!(
        repo.instance_operations(Some(operation.id()), 1)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        repo.instance_operations(None, 0).unwrap_err().code(),
        ErrorCode::LimitInvalid
    );
    assert_eq!(
        repo.prepare_instance_operation(
            &first,
            operation.id(),
            WorkflowOperationKind::Restart,
            &limits,
            10
        )
        .unwrap(),
        operation
    );
    assert_eq!(
        repo.prepare_instance_operation(
            &first,
            open_compute_core::WorkflowOperationId::generate(),
            WorkflowOperationKind::Purge,
            &limits,
            10
        )
        .unwrap_err()
        .code(),
        ErrorCode::WorkflowInstanceBusy
    );
    assert_eq!(
        repo.reserve_instance(account, definition.id, None, 2, &limits, 10)
            .unwrap_err()
            .code(),
        ErrorCode::WorkflowStateQuotaExceeded
    );
    storage
        .db()
        .with_read(|conn| {
            for sql in [
                "UPDATE workflow_instance_referrers SET instance_generation=2",
                "UPDATE workflow_instance_referrers SET state='live',instance_generation=2",
                "UPDATE workflow_instance_operations SET target_generation=3",
                "DELETE FROM workflow_instance_operations",
                "DELETE FROM workflow_instance_referrers",
                "DELETE FROM workflow_referrers",
                "DELETE FROM deployment_referrers WHERE kind='workflow_instance'",
            ] {
                assert!(conn.execute(sql, []).is_err(), "{sql}");
            }
            Ok(())
        })
        .unwrap();
    repo.verify_catalog().unwrap();
}

#[test]
fn operation_finalize_is_atomic_and_idempotent_and_purge_releases_external_identity() {
    let (_temp, storage, deployment) = setup();
    let definition = v2(&storage, deployment);
    let account = storage.identity().default_account_id;
    let repo = WorkflowRepository::new(storage.db());
    let limits = WorkflowsConfig::default();
    let identity = repo
        .reserve_instance(account, definition.id, Some("reusable"), 2, &limits, 3)
        .unwrap()
        .identity;
    repo.finalize_instance(&identity, 4).unwrap();
    let operation = repo
        .prepare_instance_operation(
            &identity,
            open_compute_core::WorkflowOperationId::generate(),
            WorkflowOperationKind::Restart,
            &limits,
            5,
        )
        .unwrap();
    // This unit test supplies the storage-private proof. Product tests must obtain it from
    // committed real scheduler state, not treat this control-side test as saga evidence.
    let proof = WorkflowAppliedOperation {
        operation: operation.clone(),
    };
    storage.db().with_read(|conn| {conn.execute_batch("CREATE TEMP TRIGGER reject_restart AFTER UPDATE OF instance_generation ON workflow_instance_referrers BEGIN SELECT RAISE(ABORT,'test fault'); END;").unwrap();Ok(())}).unwrap();
    assert!(repo.complete_instance_operation(&proof, 6).is_err());
    assert_eq!(
        repo.instance_operation(identity.instance_id).unwrap(),
        Some(operation)
    );
    assert_eq!(
        repo.reservation(identity.instance_id)
            .unwrap()
            .unwrap()
            .state,
        WorkflowRefState::Restarting
    );
    storage
        .db()
        .with_read(|conn| {
            conn.execute_batch("DROP TRIGGER reject_restart").unwrap();
            Ok(())
        })
        .unwrap();
    repo.complete_instance_operation(&proof, 6).unwrap();
    repo.complete_instance_operation(&proof, 7).unwrap();
    let next = repo
        .reservation(identity.instance_id)
        .unwrap()
        .unwrap()
        .identity;
    let mut expected = identity.clone();
    expected.instance_generation = 2;
    assert_eq!(next, expected);
    assert!(
        repo.instance_operation(identity.instance_id)
            .unwrap()
            .is_none()
    );
    assert_eq!(
        repo.prepare_instance_operation(
            &next,
            open_compute_core::WorkflowOperationId::generate(),
            WorkflowOperationKind::Purge,
            &limits,
            8
        )
        .unwrap_err()
        .code(),
        ErrorCode::WorkflowInstanceStateConflict
    );
    repo.retain_instance(&next, 8).unwrap();
    let purge = repo
        .prepare_instance_operation(
            &next,
            open_compute_core::WorkflowOperationId::generate(),
            WorkflowOperationKind::Purge,
            &limits,
            9,
        )
        .unwrap();
    assert_eq!(purge.target_generation(), 2);
    let proof = WorkflowAppliedOperation {
        operation: purge.clone(),
    };
    storage.db().with_read(|conn| {conn.execute_batch("CREATE TEMP TRIGGER reject_purge BEFORE DELETE ON workflow_instance_referrers BEGIN SELECT RAISE(ABORT,'test fault'); END;").unwrap();Ok(())}).unwrap();
    assert!(repo.complete_instance_operation(&proof, 10).is_err());
    assert_eq!(
        repo.instance_operation(next.instance_id).unwrap(),
        Some(purge)
    );
    assert!(repo.instance_referrers_intact(&next).unwrap());
    assert_eq!(
        repo.reservation(next.instance_id).unwrap().unwrap().state,
        WorkflowRefState::Retained
    );
    storage
        .db()
        .with_read(|conn| {
            conn.execute_batch("DROP TRIGGER reject_purge").unwrap();
            Ok(())
        })
        .unwrap();
    repo.complete_instance_operation(&proof, 10).unwrap();
    repo.complete_instance_operation(&proof, 11).unwrap();
    assert!(repo.reservation(next.instance_id).unwrap().is_none());
    assert!(!repo.instance_referrers_intact(&next).unwrap());
    assert!(repo.instance_operations(None, 100).unwrap().is_empty());
    let reused = repo
        .reserve_instance(account, definition.id, Some("reusable"), 2, &limits, 12)
        .unwrap()
        .identity;
    assert_ne!(reused.instance_id, identity.instance_id);
    assert_ne!(reused.creation_nonce, identity.creation_nonce);
    repo.verify_catalog().unwrap();
}
